//! Error types ported from pgBackRest `src/common/error`.
//!
//! The error-code table is **generated at build time** by `build.rs` from the upstream definition of
//! record, `/root/pgbackrest/build/error.yaml` — the same file the upstream build task renders into
//! `src/common/error/error.auto.h` and `error.auto.c.inc`. That keeps numeric codes locked to
//! upstream (AGENTS.md R5: exit codes and error codes are a compatibility surface), so adding an
//! error to the yaml is enough to make it appear here.
//!
//! # Model
//!
//! Upstream has two concepts, and this crate keeps them separate:
//!
//! - **[`ErrorTypeDef`]** — the static description of an error type (an `ERROR_DEFINE()` row in
//!   `error.auto.c.inc`: code, name, fatal). Upstream lookups are position- or name-based, so the
//!   equivalents here are [`error_type_by_code`], [`error_type_by_name`] and [`error_types`].
//! - **[`Error`]** — a thrown instance, carrying the error type plus the throw location and message
//!   (`error.c:70-82`; `errorFileName()`/`errorFunctionName()`/`errorFileLine()`/`errorMessage()`).
//!
//! # Propagation instead of a TRY/CATCH interpreter
//!
//! The C `TRY_BEGIN()/CATCH()/CATCH_ANY()/FINALLY()/TRY_END()` machinery exists because C can neither
//! return a value nor unwind ([`error.h:1-30`](https://github.com/pgbackrest/pgbackrest)). Rust has
//! both, so the semantics are expressed directly. The macro behavior that has to survive is preserved:
//!
//! - A fatal error type cannot be caught by a non-fatal handler (`error.c:307`:
//!   `assert(fatalCatch || !errorTypeFatal(errorTypeCatch))`), which [`Error::catchable_by`] reports.
//! - Errors propagate to the nearest handler in the call stack; an error that reaches no handler at all
//!   is written to stderr and exits with `unhandled` (124) — [`report_uncaught`].
//! - The exit status *is* the error code (`exitSafe()` does `result = errorCode()`), so
//!   [`Error::to_exit_status`] returns the code and [`error_type_from_code`] reproduces upstream's
//!   fallback to `unknown` (125) for a code that has no type.
//!
//! # Message formatting
//!
//! [`Error::throw`] takes an already-formatted message and caps it at [`ERROR_MESSAGE_MAX`] the way
//! `errorInternalThrowFmt()` clamps with `snprintf()`. [`Error::throw_sys`] appends the system error
//! exactly as `errorInternalThrowSys()` does: `{message}: [{errNo}] {strerror}` for a non-zero errno,
//! and the bare message for errno 0.
//!
//! [`crate_types::EncodeError`] lives below this crate in the dependency order (the type primitives
//! cannot depend on errors that come from them), so it carries the same message text and code rather
//! than throwing directly; [`Error::from_encode_error`] converts it into a real `format` error.
//!
//! # Assumptions (AGENTS.md section 5)
//!
//! - **Stack traces.** [`Error::stack_trace`] returns an empty string unless [`set_stack_trace_hook`]
//!   installed a provider. Upstream is built two ways: with libbacktrace, and without
//!   (`HAVE_LIBBACKTRACE` undefined), where `stackTraceBegin()` records only a fixed block and the
//!   trace is empty until a try frame is pushed. The trace recorder belongs to
//!   `src/common/stackTrace.c`'s own crate, which arrives with the protocol work, so the hook keeps
//!   this crate dependency-free and lets the round-trip test drive both builds.
//! - **Error context means the throw location.** Upstream no longer has free-text error context:
//!   `errorContextAdd()`/`TEST_ERROR_CONTEXT` are gone from this tree, so there is nothing to
//!   reproduce there and no replacement is invented (AGENTS.md section 9: no modernizing). The
//!   context that does exist — file, function, line — is carried by [`Error`].
//! - **`strerror` text.** Rust's `std::io::Error` message for a raw OS error renders as
//!   `{strerror} (os error {n})`, i.e. it embeds the same libc string `strerror()` gives pgBackRest.
//!   The ` (os error {n})` suffix is a std addition. Reproducing the C byte-for-byte would need a
//!   errno→string table, and a system-error message never crosses the compatibility surface (it is
//!   log text, not repository content), so this is recorded as an approved-pending deviation rather
//!   than silently reformatting messages here.

use std::sync::OnceLock;

use thiserror::Error;

/// Maximum message length, i.e. `ERROR_MESSAGE_BUFFER_SIZE - 1` (`error.c:42-46`).
///
/// Upstream formats the message into a 1 KiB static buffer and lets `snprintf()` truncate into it, so
/// the last byte is reserved for the terminator.
pub const ERROR_MESSAGE_MAX: usize = 1024 - 1;

/// Static description of an error type, i.e. one `ERROR_DEFINE()` row in `error.auto.c.inc`.
///
/// Upstream `ErrorType` (`error.c:48-58`) also stores a parent type pointer; every row at this tip has
/// `RuntimeError` as its parent, so the hierarchy is not modeled until a non-`Runtime` parent exists
/// upstream.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ErrorTypeDef {
    /// Numeric code, identical to upstream and used directly as the process exit status.
    pub code: i32,
    /// Name as written in `build/error.yaml`, i.e. what upstream `errorTypeName()` returns.
    pub name: &'static str,
    /// C identifier generated into `error.auto.h` (`file-open` -> `FileOpenError`), for parity checks.
    pub ident: &'static str,
    /// Upstream `fatal`: the code itself is wrong, so only `CATCH_FATAL()` may handle it.
    pub fatal: bool,
}

// Generated by build.rs from build/error.yaml. Provides ERROR_TYPE_LIST, ERROR_TYPE_TOTAL, and the
// `code`/`name` modules with one constant per upstream error type.
include!(concat!(env!("OUT_DIR"), "/error_code_table.rs"));

/// A thrown error: an error type plus its message and throw location.
#[derive(Clone, Debug, Error)]
#[error("{}: {message}", error_type.name)]
pub struct Error {
    /// The error type that was thrown.
    error_type: ErrorTypeDef,
    /// Message with the system error appended, as upstream stores it.
    message: String,
    /// Source file of the throw, upstream `errorFileName()`.
    file: &'static str,
    /// Function of the throw, upstream `errorFunctionName()`.
    function: &'static str,
    /// Line of the throw, upstream `errorFileLine()`.
    line: u32,
}

impl Error {
    /// Throw an error with `message`, recording the throw location the way `THROW()` uses
    /// `__FILE__`, `__func__` and `__LINE__`.
    #[track_caller]
    pub fn throw(error_type: ErrorTypeDef, message: impl Into<String>) -> Self {
        let location = std::panic::Location::caller();
        let message = message.into();

        Self {
            error_type,
            message: clamp_message(&message),
            file: location.file(),
            // Stable Rust has no API to read the enclosing function name from a Location. Upstream
            // __func__ is approximated as "" here; the exact name is retained when errors are
            // constructed explicitly via Error::at() or re-raised over the protocol.
            function: "",
            line: location.line(),
        }
    }

    /// Throw an error for a system failure, formatting the message the way
    /// `errorInternalThrowSys()`/`errorInternalThrowSysFmt()` do (`error.c:412-470`).
    ///
    /// `err_no` is an `i32` because that is the C type passed to `strerror()`; `0` means "no system
    /// error", which upstream renders as the bare message.
    #[track_caller]
    pub fn throw_sys(error_type: ErrorTypeDef, err_no: i32, message: impl Into<String>) -> Self {
        let message = message.into();
        let message = if err_no == 0 {
            message
        } else {
            // Upstream: snprintf(messageBuffer, SIZE, "%s: [%d] %s", message, errNo, strerror(errNo))
            format!(
                "{message}: [{err_no}] {}",
                std::io::Error::from_raw_os_error(err_no)
            )
        };

        Self::throw(error_type, message)
    }

    /// Construct an error at an explicit location, for tests and for re-raising a code or message
    /// received over the protocol, where the C location is not the Rust caller.
    #[must_use]
    pub fn at(
        error_type: ErrorTypeDef,
        message: impl Into<String>,
        file: &'static str,
        function: &'static str,
        line: u32,
    ) -> Self {
        Self {
            error_type,
            message: message.into(),
            file,
            function,
            line,
        }
    }

    /// Convert a [`crate_types::EncodeError`] into the upstream error it stands for.
    #[must_use]
    pub fn from_encode_error(error: crate_types::EncodeError) -> Self {
        Self::throw(
            error_type_from_code(error.code()),
            error.message().to_string(),
        )
    }

    /// The error type. Mirrors `errorType()`.
    #[must_use]
    pub const fn error_type(&self) -> ErrorTypeDef {
        self.error_type
    }

    /// The numeric code. Mirrors `errorCode()`/`errorTypeCode()` (`error.c:104-116`).
    #[must_use]
    pub const fn code(&self) -> i32 {
        self.error_type.code
    }

    /// The error name, e.g. `file-open`. Mirrors `errorName()`/`errorTypeName()`.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        self.error_type.name
    }

    /// Upstream `errorTypeFatal()`: true for a type whose code is wrong, e.g. `assert`.
    #[must_use]
    pub const fn fatal(&self) -> bool {
        self.error_type.fatal
    }

    /// The message, as upstream `errorMessage()` returns it: already includes the appended system
    /// error and already clamped to [`ERROR_MESSAGE_MAX`].
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Source file of the throw. Mirrors `errorFileName()`.
    #[must_use]
    pub const fn file(&self) -> &'static str {
        self.file
    }

    /// Function of the throw. Mirrors `errorFunctionName()`.
    #[must_use]
    pub const fn function(&self) -> &'static str {
        self.function
    }

    /// Line of the throw. Mirrors `errorFileLine()`.
    #[must_use]
    pub const fn line(&self) -> u32 {
        self.line
    }

    /// Whether a `CATCH(error_type)`-style handler may catch this error.
    ///
    /// Upstream `assert()`s that a fatal error is never caught by a non-fatal handler
    /// (`error.c:307`), so `false` means the error must keep propagating instead of being swallowed.
    /// `handler_fatal` is the `CATCH_FATAL()` flag.
    #[must_use]
    pub const fn catchable_by(&self, handler_fatal: bool) -> bool {
        handler_fatal || !self.error_type.fatal
    }

    /// The exit status this error produces, i.e. its code.
    ///
    /// Upstream `exitSafe()` sets `result = errorCode()` (`src/command/exit.c:171`) and the yaml codes
    /// are chosen to be the documented exit statuses, so the code *is* the status.
    #[must_use]
    pub const fn to_exit_status(&self) -> i32 {
        self.error_type.code
    }

    /// The stack trace captured when this error was thrown, or an empty string when no provider is
    /// installed (see the crate notes). Mirrors `errorStackTrace()` (`error.c:433-441`), which likewise
    /// returns `""` when there is nothing recorded.
    #[must_use]
    pub fn stack_trace(&self) -> String {
        stack_trace_hook().map_or_else(String::new, |hook| (hook)(self))
    }
}

/// Result type used across PgBackRust, mirroring the upstream "anything may throw" model.
pub type Result<T> = std::result::Result<T, Error>;

/// Clamp a message to the upstream message buffer, byte-for-byte.
///
/// `errorInternalThrowFmt()` truncates with `snprintf()` at `ERROR_MESSAGE_BUFFER_SIZE - 1` bytes
/// (`error.c:450-462`). The C leaves an invalid UTF-8 tail; a Rust `String` must stay valid, so the
/// clamp walks back to a character boundary. The result is never longer than upstream's and is
/// byte-identical for ASCII messages, which is everything upstream produces.
fn clamp_message(message: &str) -> String {
    if message.len() <= ERROR_MESSAGE_MAX {
        return message.to_string();
    }

    let mut end = ERROR_MESSAGE_MAX;

    while end > 0 && !message.is_char_boundary(end) {
        end -= 1;
    }

    message[..end].to_string()
}

/// Look up an error type by code, with upstream's fallback.
///
/// Mirrors `errorTypeFromCode()` (`error.c:127-146`), which linear-searches `errorTypeList[]` and
/// returns `&UnknownError` when nothing matches. A code is therefore never "not found"; the fallback is
/// the upstream behavior and is reproduced rather than turned into an error.
#[must_use]
pub fn error_type_from_code(code: i32) -> ErrorTypeDef {
    error_type_by_code(code).unwrap_or_else(|| expect_type("unknown"))
}

/// Look up an error type by code, without the `unknown` fallback.
#[must_use]
pub fn error_type_by_code(code: i32) -> Option<ErrorTypeDef> {
    ERROR_TYPE_LIST
        .iter()
        .copied()
        .find(|entry| entry.code == code)
}

/// Look up an error type by its yaml name, e.g. `file-open`.
///
/// Mirrors `errorTypeFromName()`, which raises `AssertError` for a name the table does not have — so
/// the error returned here is that `assert` error.
///
/// # Errors
/// [`Error`] with name `assert` when `name` is not declared in `build/error.yaml`.
pub fn error_type_by_name(name: &str) -> std::result::Result<ErrorTypeDef, Error> {
    ERROR_TYPE_LIST
        .iter()
        .copied()
        .find(|entry| entry.name == name)
        .ok_or_else(|| {
            Error::throw(
                expect_type("assert"),
                format!("unable to find error type '{name}'"),
            )
        })
}

/// Throw an error by numeric code, as upstream `THROW_CODE()` does through `errorTypeFromCode()`.
///
/// An unknown code becomes `unknown` (125) rather than failing loudly at the throw site, matching
/// upstream. Callers that must reject an unknown code use [`error_type_by_code`] or
/// [`error_type_by_name`] explicitly.
#[track_caller]
#[must_use]
pub fn throw_code(code: i32, message: impl Into<String>) -> Error {
    Error::throw(error_type_from_code(code), message)
}

/// All error types in upstream declaration order, i.e. the generated `errorTypeList[]`.
#[must_use]
pub fn error_types() -> &'static [ErrorTypeDef] {
    ERROR_TYPE_LIST
}

/// Fetch a generated error type by yaml name at compile time.
///
/// Panics only if `build.rs` produced a table without `name`, which the build script rules out. This
/// exists for callers that need an error type in a `const` context, e.g. `crate-log` reproducing the
/// `assert()` of `logLevelEnum()`.
#[must_use]
pub const fn error_type(name: &str) -> ErrorTypeDef {
    let list = ERROR_TYPE_LIST;
    let mut idx = 0;

    while idx < list.len() {
        if bytes_eq(list[idx].name.as_bytes(), name.as_bytes()) {
            return list[idx];
        }

        idx += 1;
    }

    panic!("build.rs must generate every error declared in error.yaml")
}

/// Byte comparison usable in a `const fn` (`str::eq` is not const).
const fn bytes_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }

    let mut idx = 0;

    while idx < left.len() {
        if left[idx] != right[idx] {
            return false;
        }

        idx += 1;
    }

    true
}

/// Fetch a generated error type by yaml name, panicking only if `build.rs` produced a broken table.
fn expect_type(name: &str) -> ErrorTypeDef {
    error_type(name)
}

/// Hook that renders a stack trace for a thrown error, installed by the stack-trace crate.
type StackTraceHook = dyn Fn(&Error) -> String + Send + Sync;

static STACK_TRACE_HOOK: OnceLock<&'static StackTraceHook> = OnceLock::new();

/// Install the stack trace provider used by [`Error::stack_trace`].
///
/// A hook rather than a dependency: `crate-error` is the lowest-level crate and the recorder belongs to
/// `src/common/stackTrace.c`'s future crate. Calling it twice keeps the first provider, which matches
/// upstream initializing the trace once per process.
pub fn set_stack_trace_hook(hook: &'static StackTraceHook) {
    let _ = STACK_TRACE_HOOK.set(hook);
}

fn stack_trace_hook() -> Option<&'static StackTraceHook> {
    STACK_TRACE_HOOK.get().copied()
}

/// Render what upstream prints when an error escapes every `TRY` block, byte for byte
/// (`errorInternalPropagate()`, `error.c:335-347`).
#[must_use]
pub fn uncaught_message(error: &Error) -> String {
    format!(
        "\nUncaught {}: {}\n    thrown at {}:{}\n\n",
        error.name(),
        error.message(),
        error.file(),
        error.line()
    )
}

/// Report an uncaught error and return the status upstream would exit with.
///
/// Upstream writes the block to stderr, flushes, and exits `unhandled` (124), bypassing `exitSafe()`
/// and lock release (`error.c:335-347`).
pub fn report_uncaught(error: &Error) -> i32 {
    // Upstream fflush()s stderr after the fprintf(); eprint! writes unbuffered
    eprint!("{}", uncaught_message(error));
    expect_type("unhandled").code
}
