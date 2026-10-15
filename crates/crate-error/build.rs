//! Build script for `crate-error`: generates the error-code table from the upstream definition of
//! record, `/root/pgbackrest/build/error.yaml`.
//!
//! This is the Rust equivalent of the upstream build task
//! `/root/pgbackrest/build/lib/exec/error.yaml` → `build/lib/error/render.py`, which renders
//! `src/common/error/error.auto.h` and `src/common/error/error.auto.c.inc`. Parity points:
//!
//! - **Declaration order is the order.** `build/lib/error/parse.py:1-6`: "The order they are
//!   declared in is the order they are rendered in, since the generated array is indexed by nothing
//!   but position". The emitted [`ERROR_TYPE_LIST`] keeps that order.
//! - **Numeric codes are copied verbatim** from the yaml, so a code can only drift if upstream
//!   changes it (and then the daily upstream sync duty, AGENTS.md section 7, notices).
//! - **Name mangling** is `bld_enum()` from `build/lib/common/render.py:39-59` plus the `"Error"`
//!   suffix from `build/lib/error/render.py:17-21`: a dash is deleted and upper-cases only the
//!   character immediately after it, e.g. `file-open` → `FileOpenError`.
//! - **Validation** mirrors `build/lib/error/parse.py`: `CODE_MIN`/`CODE_MAX` are 25/125, a
//!   non-integer code, a missing code, an unknown definition key, and a non-boolean `fatal` are all
//!   hard failures that stop the build, and the messages are byte-identical to the upstream
//!   `ToolError` messages so a bad yaml fails the same way in both tools.
//!
//! ## YAML subset
//!
//! Upstream reads the file with PyYAML `SafeLoader` minus implicit typing (`build/lib/common/yaml.py`),
//! so every scalar is text and mappings are ordered pair lists. The parser below accepts the subset
//! that file actually uses: block mappings, one nesting level, `#` comments, blank lines, and bare
//! or single/double-quoted keys and scalars. It is deliberately strict: anything it does not
//! understand aborts the build rather than being silently skipped, so the generated table can never
//! disagree with what the upstream tool would have produced.

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

/// Error codes an error may use. Mirrors `CODE_MIN`/`CODE_MAX` in `build/lib/error/parse.py:15-16`.
const CODE_MIN: i32 = 25;
const CODE_MAX: i32 = 125;

/// An error definition parsed from the yaml. Mirrors `BldErrError` in `build/lib/error/parse.py`.
struct BldErrError {
    name: String,
    code: i32,
    fatal: bool,
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let path = error_yaml_path();
    println!("cargo:rerun-if-changed={}", path.display());

    let content = fs::read_to_string(&path).unwrap_or_else(|error| {
        fail(&format!(
            "unable to read error declarations from '{}': {error}",
            path.display()
        ));
    });

    let error_list = parse_error_yaml(&content, &path);
    check_errors(&error_list);

    let out_dir = env::var("OUT_DIR").expect("OUT_DIR is always set for a build script");
    let dest = Path::new(&out_dir).join("error_code_table.rs");
    let rendered = render(&error_list, &path);

    fs::write(&dest, rendered)
        .unwrap_or_else(|error| fail(&format!("unable to write '{}': {error}", dest.display())));
}

/// Locate the upstream error declarations, i.e. the single source of truth for error codes.
///
/// `PGBACKREST_SOURCE` matches the upstream build harness convention (`--pgbackrest-source` is
/// exported by the real build, and used here by the golden tests). The default is the reference
/// clone documented in AGENTS.md section 1.
fn error_yaml_path() -> PathBuf {
    let root = env::var("PGBACKREST_SOURCE")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/root/pgbackrest"));

    let path = root.join("build/error.yaml");

    if !path.is_file() {
        fail(&format!(
            "upstream error declarations not found at '{}' \
             (set PGBACKREST_SOURCE to the pgBackRest reference tree)",
            path.display()
        ));
    }

    path
}

/// Abort the build with the message the upstream build tool would report.
fn fail(message: &str) -> ! {
    // Upstream renders a tool failure as "unable to execute build: {message}" on stderr
    // (build/lib/exec/error.yaml:121-124).
    eprintln!("Error: unable to execute build: {message}");
    eprintln!("build failed");
    std::process::exit(1);
}

/// `bld_enum(None, value)` / `bld_enum(Some(""), value)` from `build/lib/common/render.py:39-59`.
///
/// A dash is deleted and upper-cases the character that follows it; every other character is passed
/// through unchanged. Only the character immediately after a dash is capitalized.
fn bld_enum(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut upper = true; // prefix is not None for _err_name(), so the first char is upper-cased

    for char in value.chars() {
        if char == '-' {
            upper = true;
            continue;
        }

        if upper {
            result.extend(char.to_uppercase());
        } else {
            result.push(char);
        }

        upper = false;
    }

    result
}

/// `_err_name()` from `build/lib/error/render.py:17-21`: "file-open" becomes "FileOpenError".
fn err_name(name: &str) -> String {
    bld_enum(name) + "Error"
}

/// A `key: value` line split into its halves, with comments and quotes removed.
///
/// Returns `None` only for a blank line, a comment line, or a document marker. Because upstream
/// disables implicit typing (`build/lib/common/yaml.py:48`), values stay text and the caller
/// converts them. A content line without a colon aborts the build rather than being skipped, so a
/// generated table can never silently omit an entry.
fn split_key_value<'a>(line: &'a str, path: &'a Path) -> Option<(&'a str, &'a str, usize)> {
    let trimmed = line.trim_end();

    // A comment line has no content; '#' starts a comment when it is the first non-space character
    let body = trimmed.strip_prefix(char::is_whitespace).unwrap_or(trimmed);
    if body.is_empty() || body.starts_with('#') || body == "---" || body == "..." {
        return None;
    }

    // Comments may only be stripped outside a quoted scalar (YAML rule), so scan for an unquoted '#'
    let body = strip_comment(body);
    let indent = body.len() - body.trim_start().len();
    let content = body.trim();

    let Some((key, value)) = content.split_once(':') else {
        fail(&format!(
            "'{}': expected a key, found '{content}'",
            path.display()
        ));
    };

    Some((unquote(key.trim()), unquote(value.trim()), indent))
}

/// Remove a trailing `# comment` that is not inside quotes.
fn strip_comment(body: &str) -> &str {
    let bytes = body.as_bytes();
    let mut in_single = false;
    let mut in_double = false;
    let mut idx = 0;

    while idx < bytes.len() {
        match bytes[idx] {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b'#' if !in_single
                && !in_double
                && (idx == 0 || bytes[idx - 1].is_ascii_whitespace()) =>
            {
                // '#' only starts a comment when preceded by whitespace or at the start of the line
                return &body[..idx];
            }
            _ => {}
        }

        idx += 1;
    }

    body
}

fn unquote(value: &str) -> &str {
    for quote in ['\'', '"'] {
        if value.len() >= 2 && value.starts_with(quote) && value.ends_with(quote) {
            return &value[1..value.len() - 1];
        }
    }

    value
}

/// One yaml key/value pair with its indentation, in file order.
struct Item<'a> {
    indent: usize,
    key: &'a str,
    value: &'a str,
}

/// Extract the `key: value` pairs of the block mapping, skipping comments and blank lines.
fn items<'a>(content: &'a str, path: &'a Path) -> Vec<Item<'a>> {
    let mut result = Vec::new();

    for line in content.lines() {
        if let Some((key, value, indent)) = split_key_value(line, path) {
            result.push(Item { indent, key, value });
        }
    }

    result
}

/// Parse `build/error.yaml` the way `bld_err_parse()` does (`build/lib/error/parse.py:44-69`).
///
/// The upstream loop walks the ordered top-level map and, for each entry, either takes a scalar as
/// the code or reads a nested map of `code`/`fatal`. The same shape is expressed here over the flat
/// item list: an entry starts at indentation 0 and its detail items are the following more-indented
/// lines.
fn parse_error_yaml(content: &str, path: &Path) -> Vec<BldErrError> {
    let item_list = items(content, path);
    let mut result: Vec<BldErrError> = Vec::new();
    let mut idx = 0;

    while idx < item_list.len() {
        let item = &item_list[idx];

        if item.indent != 0 {
            fail(&format!(
                "'{}': unexpected indentation at '{key}'",
                path.display(),
                key = item.key
            ));
        }

        if let Some(seen) = result.iter().find(|entry| entry.name == item.key) {
            fail(&format!(
                "'{}' has duplicate key '{}'",
                path.display(),
                seen.name
            ));
        }

        // Collect the detail items of this entry: everything indented deeper than the entry key
        let mut code: Option<String> = None;
        let mut fatal: Option<String> = None;
        idx += 1;

        while idx < item_list.len() && item_list[idx].indent > item.indent {
            let detail = &item_list[idx];

            match detail.key {
                // A scalar detail means `name: code`, which the caller already handled when
                // item.value is non-empty; a nested map gives `code:`/`fatal:` keys
                "code" => {
                    check_unused(&code, detail.key, path);
                    code = Some(detail.value.to_string());
                }
                "fatal" => {
                    check_unused(&fatal, detail.key, path);
                    fatal = Some(detail.value.to_string());
                }
                _ => fail(&format!(
                    "'{}': unknown error definition '{}'",
                    path.display(),
                    detail.key
                )),
            }

            idx += 1;
        }

        if !item.value.is_empty() {
            // A scalar is the code on its own
            check_unused(&code, "code", path);
            code = Some(item.value.to_string());
        }

        result.push(finish_entry(item.key.to_string(), code, fatal));
    }

    if result.is_empty() {
        fail(&format!("'{}' does not declare any errors", path.display()));
    }

    result
}

/// Upstream `yaml_map_dict()` rejects a duplicate key with "'{path}' has duplicate key '{key}'".
fn check_unused(existing: &Option<String>, key: &str, path: &Path) {
    if existing.is_some() {
        fail(&format!("'{}' has duplicate key '{key}'", path.display(),));
    }
}

/// Turn the parsed detail of one entry into a `BldErrError`, applying `_code_parse()` semantics.
fn finish_entry(name: String, code: Option<String>, fatal: Option<String>) -> BldErrError {
    let Some(code) = code else {
        fail(&format!("error '{name}' requires a code"));
    };

    let parsed = code.parse::<i32>().unwrap_or_else(|_| {
        fail(&format!("error '{name}' code '{code}' is not an integer"));
    });

    if !(CODE_MIN..=CODE_MAX).contains(&parsed) {
        fail(&format!(
            "error '{name}' code must be >= {CODE_MIN} and <= {CODE_MAX}"
        ));
    }

    let fatal = match fatal.as_deref() {
        None => false,
        // yaml_bool() (`build/lib/common/yaml.py:71-77`) accepts only the exact strings
        Some("true") => true,
        Some("false") => false,
        Some(value) => fail(&format!(
            "invalid boolean '{value}' for error '{name}' fatal"
        )),
    };

    BldErrError {
        name,
        code: parsed,
        fatal,
    }
}

/// Post-parse invariants. A duplicate code or name cannot be represented by the upstream lookup
/// functions without losing an error type, so it must fail the build.
fn check_errors(error_list: &[BldErrError]) {
    for (idx, entry) in error_list.iter().enumerate() {
        for other in &error_list[idx + 1..] {
            if entry.code == other.code {
                fail(&format!(
                    "error '{}' and '{}' have the same code {}",
                    entry.name, other.name, entry.code
                ));
            }

            if entry.name == other.name {
                fail(&format!("error '{}' is declared twice", entry.name));
            }
        }
    }
}

/// Render the generated table. Emitted with the entries in declaration order.
fn render(error_list: &[BldErrError], path: &Path) -> String {
    let mut out = String::new();

    out.push_str(&format!(
        "// Generated by build.rs from {} -- DO NOT EDIT.\n\
         //\n\
         // Equivalent to upstream src/common/error/error.auto.h + error.auto.c.inc. See build.rs\n\
         // for the parity rules that produced this file.\n\n",
        path.display()
    ));

    out.push_str(
        "/// Every error type in declaration order, i.e. the upstream `errorTypeList[]`.\n",
    );
    out.push_str("pub static ERROR_TYPE_LIST: &[crate::ErrorTypeDef] = &[\n");

    for entry in error_list {
        let mut line = String::new();
        writeln!(
            &mut line,
            "    crate::ErrorTypeDef {{ code: {}, name: \"{}\", ident: \"{}\", fatal: {} }},",
            entry.code,
            entry.name,
            err_name(&entry.name),
            entry.fatal
        )
        .expect("writing to a String cannot fail");
        out.push_str(&line);
    }

    out.push_str("];\n\n");

    out.push_str("/// Total number of error types declared upstream.\n");
    writeln!(
        out,
        "pub const ERROR_TYPE_TOTAL: usize = {};\n",
        error_list.len()
    )
    .expect("writing to a String cannot fail");

    // Per-error code constants, so Rust code can reference a code the way C references AssertError.code
    out.push_str("/// Error codes, one constant per upstream error type.\n");
    out.push_str("pub mod code {\n");

    for entry in error_list {
        writeln!(
            out,
            "    /// `{}` -> upstream `{}`.",
            entry.name,
            err_name(&entry.name)
        )
        .expect("writing to a String cannot fail");
        writeln!(
            out,
            "    pub const {}: i32 = {};",
            entry.name.replace('-', "_").to_uppercase(),
            entry.code
        )
        .expect("writing to a String cannot fail");
    }

    out.push_str("}\n\n");

    // Per-error name constants (upstream errorTypeName() result, i.e. the yaml key)
    out.push_str("/// Error names as reported by upstream `errorTypeName()`.\n");
    out.push_str("pub mod name {\n");

    for entry in error_list {
        writeln!(out, "    /// Upstream `{}`.", err_name(&entry.name))
            .expect("writing to a String cannot fail");
        writeln!(
            out,
            "    pub const {}: &str = \"{}\";",
            entry.name.replace('-', "_").to_uppercase(),
            entry.name
        )
        .expect("writing to a String cannot fail");
    }

    out.push_str("}\n");

    out
}
