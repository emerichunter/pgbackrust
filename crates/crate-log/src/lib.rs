//! # crate-log — pgBackRest `src/common/log.{c,h}` and `src/common/logLevel.h`
//!
//! A byte-exact port of pgBackRest's log handler: the level table, the log-line format, the destination
//! routing, and the buffer clamps. `log.c` is the specification, and every rule here carries the line it
//! came from.
//!
//! ## What a line looks like
//!
//! `logPre()` (`log.c:341-411`) assembles a header, then `logInternal()` (`log.c:525-545`) appends the
//! message, then `logPost()` (`log.c:418-479`) terminates the line and picks destinations:
//!
//! ```text
//! 2026-10-15 08:54:44.109 P044  ERROR: [064]: unable to write log to file
//! └──────── timestamp ───┘└─┐ └──┐ └─┐ └─┐ └┬┘└────────── message ──────┘
//!                      "24" P "044" " " "ERROR" ":" " "
//! ```
//!
//! - `P%0*u ` — `P`, the process id zero-padded to 2 or 3 digits, then one space
//!   (`"P%0*u %*s: "`, `log.c:377`).
//! - `%6s: ` — the level name right-padded to 6 characters, then `": "` (`log.c:379`; the `6` is a
//!   literal there, so it is a literal here, see [`format::LOG_LEVEL_WIDTH`]).
//! - `[%03d]: ` — only when the level is `error` or `assert` and `code != 0` (`log.c:387-389`).
//! - `[DRY-RUN] ` — only with dry run on and a `DEBUG`-or-lower level (`log.c:391-395`).
//! - debug info — only for `DEBUG`/`TRACE`: 4 or 8 spaces of padding, then `file::function: ` with the
//!   file name's last two characters dropped (`log.c:395-408`).
//!
//! `log-level-stderr` and `log-level-file` decide where the line goes; stderr gets the line from the level
//! name onward, with the timestamp, the process id and the level padding dropped (`log.c:381-382`,
//! `log.c:450`). Continuation lines of a multi-line message are indented under the message column, and
//! stderr's indent is smaller than stdout's by the width of the prefix it skipped.
//!
//! ## Layout
//!
//! | module | upstream |
//! |---|---|
//! | [`level`] | `logLevel.h:11-21`, `log.c:55-97`, `log.h:13-20` |
//! | [`format`] | `log.c:330-416` (`logPre`) and the `logBuffer` a message lands in |
//! | [`handler`] | `log.c:22-49` (state), `105-264`, `418-545` (`logPost`/`logSignal`/`logInternal`) |
//! | [`capture`] | nothing — the test-only [`Console`](handler::Console) |
//! | [`time`] | `common/time.c`'s `timeMSec` plus libc `localtime_r`/`strftime` |
//!
//! ## Deviations from upstream, all pending user approval
//!
//! These are the only places this port does not emit what `log.c` emits. Each is also recorded in
//! `docs/COMPATIBILITY_CONTRACT.md`, whose "Approved deviations" section is still empty.
//!
//! 1. **The zone source.** Upstream calls libc `localtime_r()`, which reads `TZ` and `/etc/localtime`.
//!    Rust has no equivalent in `std`, and a tz database reader is a new dependency (R8) needing approval,
//!    so [`time`] computes local wall-clock time from a UTC offset and defaults that offset to the one
//!    `std` exposes. With `TZ` unset — or set to a zone whose abbreviation `std` cannot resolve to an
//!    offset — that is UTC, while upstream resolves `TZ` fully. See [`time::local_offset`].
//! 2. **No `%z` anywhere.** `log.c:584` uses `%0*d` for the second field of `logLevelDebug`'s `printf`
//!    format, i.e. the numeric error code; see [`format::Header::format`].
//! 3. **`log-level-*` name parsing** lives in the config layer upstream, not in `log.c`, so
//!    [`level::LogLevel::from_name`]'s error is provisional until `crate-config` is ported.
//!
//! ## Not ported here
//!
//! `logInit()`'s callers in `main.c`, the `log.h` macros (`LOG_*`), and the fork/child-process plumbing
//! that decides which process id to pass. The macros become [`handler::log`], which takes the file and
//! function explicitly because Rust has no `__FILE__`/`__func__`.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod capture;
pub mod format;
pub mod handler;
pub mod level;
pub mod time;

pub use format::{Header, Line};
pub use level::LogLevel;

// The `LOG_*` macro surface of log.h: the level-specific entry points a caller actually uses.
//
// Upstream's macros (`log.h:118-135`) expand to `logInternal(level, LOG_LEVEL_MIN, LOG_LEVEL_MAX,
// processId, code, message)`, i.e. they never restrict the range, and the assert variant is
// `logInternal(logLevelAssert, ..., ASSERT_CODE, ...)` with `ASSERT_CODE` 25 (`assert.h:16`).
//
// These are thin wrappers over [`handler::log`] so that the process-wide handler stays behind one call.

/// `LOG_ASSERT` / `LOG_ASSERT_FMT` (`log.h:121`, `log.h:140`) — the assert level with code 25.
///
/// # Errors
/// `file-write` when a destination rejects the bytes.
pub fn log_assert(file: &str, function: &str, message: &str) -> Result<(), crate_error::Error> {
    handler::log(
        LogLevel::Assert,
        crate_error::error_type("assert").code,
        file,
        function,
        message,
    )
}

/// `LOG_ERROR` (`log.h:123`), with the error code of the `code` argument.
///
/// # Errors
/// `file-write` when a destination rejects the bytes.
pub fn log_error(
    code: u32,
    file: &str,
    function: &str,
    message: &str,
) -> Result<(), crate_error::Error> {
    handler::log(LogLevel::Error, code as i32, file, function, message)
}

/// `LOG_WARN` (`log.h:125`) — code 0, since the field is unused below `error`.
///
/// # Errors
/// `file-write` when a destination rejects the bytes.
pub fn log_warn(file: &str, function: &str, message: &str) -> Result<(), crate_error::Error> {
    handler::log(LogLevel::Warn, 0, file, function, message)
}

/// `LOG_INFO` (`log.h:127`).
///
/// # Errors
/// `file-write` when a destination rejects the bytes.
pub fn log_info(file: &str, function: &str, message: &str) -> Result<(), crate_error::Error> {
    handler::log(LogLevel::Info, 0, file, function, message)
}

/// `LOG_DETAIL` (`log.h:129`).
///
/// # Errors
/// `file-write` when a destination rejects the bytes.
pub fn log_detail(file: &str, function: &str, message: &str) -> Result<(), crate_error::Error> {
    handler::log(LogLevel::Detail, 0, file, function, message)
}

/// `LOG_DEBUG` (`log.h:131`) — code 0: `logPre()` only renders the field for `error`/`assert`.
///
/// # Errors
/// `file-write` when a destination rejects the bytes.
pub fn log_debug(file: &str, function: &str, message: &str) -> Result<(), crate_error::Error> {
    handler::log(LogLevel::Debug, 0, file, function, message)
}

/// `LOG_TRACE` (`log.h:133`).
///
/// # Errors
/// `file-write` when a destination rejects the bytes.
pub fn log_trace(file: &str, function: &str, message: &str) -> Result<(), crate_error::Error> {
    handler::log(LogLevel::Trace, 0, file, function, message)
}
