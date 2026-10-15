//! Error-code parity tests: the generated table must match the upstream yaml byte for byte.
//!
//! The generated table lives in OUT_DIR and is compiled into crate-error. These tests
//! re-parse the same upstream file the build script reads and compare every field. This is
//! the round-1 instantiation of AGENTS.md R4/R5: error codes are a compatibility surface.

use std::path::PathBuf;

use crate_error::{
    ERROR_TYPE_LIST, ERROR_TYPE_TOTAL, Error, error_type_by_code, error_type_by_name,
    error_type_from_code, throw_code,
};

/// Locate the upstream error definitions, same resolution as build.rs.
fn error_yaml_path() -> PathBuf {
    std::env::var("PGBACKREST_SOURCE")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/root/pgbackrest"))
        .join("build/error.yaml")
}

/// One entry parsed from the yaml, in declaration order.
#[derive(Clone, Debug, PartialEq, Eq)]
struct YamlEntry {
    name: String,
    code: i32,
    fatal: bool,
}

/// Parse the yaml subset upstream uses: `name: code` or `name:` with nested `code:`/`fatal:`.
fn parse_error_yaml(content: &str) -> Vec<YamlEntry> {
    let lines: Vec<&str> = content
        .lines()
        .map(str::trim_end)
        .filter(|line| {
            let body = line.trim_start();
            !body.is_empty() && !body.starts_with('#') && body != "---" && body != "..."
        })
        .collect();

    let mut entries = Vec::new();
    let mut idx = 0;

    while idx < lines.len() {
        let line = lines[idx];
        let indent = line.len() - line.trim_start().len();

        // Top-level entries only. Anything deeper than 0 without a "code:" key is an error.
        let (key, value) = split_colon(line);
        let key = unquote(key.trim());
        if value.trim().is_empty() && indent != 0 {
            panic!("unexpected indented line in error.yaml: {line}");
        }

        // Collect nested detail lines.
        let mut code: Option<String> = None;
        let mut fatal: Option<String> = None;
        idx += 1;

        while idx < lines.len() {
            let detail = lines[idx];
            let detail_indent = detail.len() - detail.trim_start().len();
            if detail_indent <= indent {
                break;
            }
            let (detail_key, detail_value) = split_colon(detail);
            match unquote(detail_key.trim()) {
                "code" => code = Some(unquote(detail_value.trim()).to_string()),
                "fatal" => fatal = Some(unquote(detail_value.trim()).to_string()),
                other => panic!("unknown error definition key: {other}"),
            }
            idx += 1;
        }

        if !value.trim().is_empty() {
            code = Some(unquote(value.trim()).to_string());
        }

        let code = code.unwrap_or_else(|| panic!("error '{key}' requires a code"));
        let code: i32 = code
            .parse()
            .unwrap_or_else(|_| panic!("bad code for '{key}'"));
        let fatal = match fatal.as_deref() {
            None | Some("false") => false,
            Some("true") => true,
            Some(other) => panic!("bad fatal for '{key}': {other}"),
        };

        entries.push(YamlEntry {
            name: key.to_string(),
            code,
            fatal,
        });
    }

    entries
}

fn split_colon(line: &str) -> (&str, &str) {
    let body = strip_comment(line);
    let content = body.trim();
    let (key, value) = content
        .split_once(':')
        .unwrap_or_else(|| panic!("no colon: {content}"));
    (key, value)
}

fn strip_comment(body: &str) -> &str {
    let bytes = body.as_bytes();
    let mut in_single = false;
    let mut in_double = false;

    for (idx, byte) in bytes.iter().enumerate() {
        match byte {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b'#' if !in_single
                && !in_double
                && (idx == 0 || bytes[idx - 1].is_ascii_whitespace()) =>
            {
                return &body[..idx];
            }
            _ => {}
        }
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

/// `bld_enum()` from upstream build/lib/common/render.py (dash deleted, next char upper-cased).
fn bld_enum(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut upper = true;

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

fn err_ident(name: &str) -> String {
    bld_enum(name) + "Error"
}

#[test]
fn parity_with_upstream_error_yaml() {
    let yaml = std::fs::read_to_string(error_yaml_path()).expect("upstream error.yaml readable");
    let entries = parse_error_yaml(&yaml);
    assert!(!entries.is_empty(), "yaml must declare errors");

    // Total and order must match exactly (declaration order is the upstream array order).
    assert_eq!(
        ERROR_TYPE_TOTAL,
        entries.len(),
        "ERROR_TYPE_TOTAL must equal yaml entry count"
    );
    assert_eq!(ERROR_TYPE_LIST.len(), entries.len());

    for (idx, entry) in entries.iter().enumerate() {
        let generated = ERROR_TYPE_LIST[idx];
        assert_eq!(
            generated.code, entry.code,
            "entry {idx} '{}': code mismatch",
            entry.name
        );
        assert_eq!(
            generated.name, entry.name,
            "entry {idx} (code {}): name mismatch",
            entry.code
        );
        assert_eq!(
            generated.ident,
            err_ident(&entry.name),
            "entry {idx} '{}': C identifier mismatch",
            entry.name
        );
        assert_eq!(
            generated.fatal, entry.fatal,
            "entry {idx} '{}': fatal mismatch",
            entry.name
        );
    }
}

#[test]
fn every_code_resolves_and_round_trips() {
    for entry in ERROR_TYPE_LIST {
        let found = error_type_by_code(entry.code)
            .unwrap_or_else(|| panic!("code {} '{}' must resolve", entry.code, entry.name));
        assert_eq!(found, *entry);
        let by_name = error_type_by_name(entry.name)
            .unwrap_or_else(|_| panic!("name '{}' must resolve", entry.name));
        assert_eq!(by_name, *entry);
    }
}

#[test]
fn unknown_code_falls_back_to_unknown() {
    // Upstream errorTypeFromCode() falls back to UnknownError (125) for an unknown code.
    let fallback = error_type_from_code(9999);
    assert_eq!(fallback.name, "unknown");
    assert_eq!(fallback.code, 125);

    // throw_code() with an unknown code throws the unknown error, not a lookup failure.
    let error = throw_code(9999, "boom");
    assert_eq!(error.code(), 125);
    assert_eq!(error.name(), "unknown");
}

#[test]
fn unknown_name_raises_assert() {
    let error = error_type_by_name("no-such-error").expect_err("unknown name must fail");
    assert_eq!(error.code(), 25, "upstream raises AssertError");
    assert_eq!(error.name(), "assert");
    assert!(error.fatal(), "assert is a fatal error type");
}

#[test]
fn message_is_clamped_to_buffer_size() {
    // errorInternalThrowFmt() clamps at ERROR_MESSAGE_BUFFER_SIZE - 1 bytes.
    let long = "x".repeat(5000);
    let error = Error::throw(
        error_type_by_name("file-open").expect("file-open exists"),
        long,
    );
    assert_eq!(error.message().len(), 1023);
}

#[test]
fn throw_sys_formats_like_c() {
    // errorInternalThrowSys(): "{message}: [{errNo}] {strerror}" for a non-zero errno.
    let error = Error::throw_sys(
        error_type_by_name("file-open").expect("file-open exists"),
        2,
        "boom",
    );
    assert!(
        error
            .message()
            .starts_with("boom: [2] No such file or directory"),
        "unexpected message: {}",
        error.message()
    );
}

#[test]
fn exit_status_is_the_code() {
    let error = throw_code(26, "checksum failure");
    assert_eq!(error.to_exit_status(), 26);
}

#[test]
fn error_renders_as_name_colon_message() {
    let error = Error::throw(
        error_type_by_name("checksum").expect("checksum exists"),
        "bad sum",
    );
    assert_eq!(error.to_string(), "checksum: bad sum");
}
