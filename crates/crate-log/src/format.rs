//! Log line formatting, ported from pgBackRest `src/common/log.c:330-479`.
//!
//! This is the byte-exact surface: [`Header`] is `logPre()`, [`Line`] is the `logBuffer` a message ends
//! up in after `logPost()`, and [`split_lines`] is `logWriteIndent()`. Every width and every format
//! string below comes from the C, with the source line quoted next to it.
//!
//! # The header, in emission order (`logPre()`, `log.c:341-411`)
//!
//! | part | C format string | bytes | line |
//! |---|---|---|---|
//! | timestamp | `"%Y-%m-%d %H:%M:%S"` then `".%03d "` | 19 + 5 = 24 | 365-374 |
//! | process + level | `"P%0*u %*s: "` | `process_size + 10` | 376-379 |
//! | error code | `"[%03d]: "` | 8 | 387-389 |
//! | dry run | `"[DRY-RUN] "` | 10 | 391-393 |
//! | debug padding | `(level - DEBUG + 1) * 4` spaces | 4 or 8 | 395-402 |
//! | debug info | `"%.*s::%s: "` with precision `strlen(fileName) - 2` | variable | 405-407 |
//!
//! Two details carry the whole compatibility burden:
//!
//! - **`indent_size` is captured right after the process/level field** (`log.c:384-385`) and then only
//!   grows by the debug padding. The timestamp, the error code and the dry-run prefix are *excluded*, so
//!   a continuation line of an `ERROR` under-indents by the width of `[026]: ` and a `WARN` with dry run
//!   enabled under-indents by 10. That is upstream behavior, not a bug to fix (AGENTS.md section 5).
//! - **the stderr view starts inside the header** (`log.c:381-382`):
//!   `logBufferStdErr = logBuffer + bufferPos - strlen(level) - 2`, i.e. it rewinds over `": "` and the
//!   level name, landing on the level name itself, so stderr loses the timestamp, the process id and the
//!   level alignment padding. `stderr_indent()` reproduces the subtraction `logPost()` applies to
//!   `indent_size` at `log.c:451`.
//!
//! # Assumptions
//!
//! - **The level column width 6 is hardcoded in the C** (`log.c:379` passes `6` as the `*` width) and is
//!   nowhere tied to the level names by a `static_assert`. It is a `LOG_LEVEL_WIDTH` constant here, and
//!   the crate tests assert every level name fits it. Upstream is safe today only because the longest
//!   name (`DETAIL`/`ASSERT`) is exactly 6; a 7-character name would make the stderr indent subtraction
//!   underflow at `log.c:451`.
//! - **`strlen(fileName) - 2` strips a two-character extension, not `.c` specifically** (`log.c:406`):
//!   `"test.c"` prints `test`, `"log.c.inc"` would print `log.c`. The precision is unguarded against a
//!   file name shorter than two characters, where C would pass a negative precision to `%.*s`. Here the
//!   saturating subtraction prints the whole name instead; no upstream source file is that short, so the
//!   two agree for every real log line.
//! - **UTF-8 vs bytes.** Upstream truncates at a byte count and can split a multi-byte character; the
//!   clamp in [`Line`] truncates at a byte count too and only walks back to a char boundary when the
//!   message is a Rust `&str`. Since every message upstream produces is ASCII, the outputs agree.

use crate::level::{LOG_LEVEL_SEQ, LogLevel};
use crate::time::CivilDateTime;

/// `LOG_BUFFER_SIZE` (`src/common/log.h:17`).
pub const LOG_BUFFER_SIZE: usize = 32 * 1024;

/// The width the level column is padded to — the `6` in `log.c:379`.
pub const LOG_LEVEL_WIDTH: usize = 6;

/// `DRY_RUN_PREFIX` (`log.c:53`).
pub const DRY_RUN_PREFIX: &str = "[DRY-RUN] ";

/// `LOG_BANNER` (`log.c:416`), written once per log file before the first message.
pub const LOG_BANNER: &str = "-------------------PROCESS START-------------------\n";

/// `LOG_SIGNAL_MESSAGE_PRE` (`log.c:481`).
pub const LOG_SIGNAL_MESSAGE_PRE: &str = "terminated on signal ";

/// `(logLevel - logLevelDebug + 1) * 4` spaces of padding for debug info (`log.c:399`).
///
/// `None` below `DEBUG`, which is where the C block is skipped entirely.
#[must_use]
pub const fn debug_padding(level: LogLevel) -> Option<usize> {
    if level.seq() >= LogLevel::Debug.seq() {
        Some((level.seq() as usize - LogLevel::Debug.seq() as usize + 1) * 4)
    } else {
        None
    }
}

/// The result of `logPre()` (`log.c:341-411`): the header, the indent width, and the stderr view of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    /// The header text, i.e. `logBuffer` up to `bufferPos`.
    text: String,
    /// Byte offset of the level name inside [`Self::text`], i.e. `logBufferStdErr - logBuffer`.
    stderr_offset: usize,
    /// `indentSize`: continuation-line indent for the full header.
    indent_size: usize,
}

impl Header {
    /// Everything `logPre()` needs. `process_id` is the resolved id, so the `UINT_MAX` sentinel from the
    /// `LOG_*` macros is already replaced by `logInit()`'s default (`log.c:378-379`).
    ///
    /// `code` is the upstream `int code`: `0` for a normal message, `25` for an assert, otherwise the
    /// error code of a `LOG_ERROR`-style call.
    // The argument list mirrors logPre()'s parameters exactly (log.c:341); upstream carries the
    // same set, so the count is deliberately not reduced.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn format(
        level: LogLevel,
        process_id: u32,
        process_size: usize,
        file_name: &str,
        function_name: &str,
        code: i32,
        timestamp: Option<CivilDateTime>,
        dry_run: bool,
    ) -> Self {
        let mut text = String::new();

        // Add time (log.c:365-374)
        if let Some(timestamp) = timestamp {
            text.push_str(&timestamp.format_log_timestamp());
        }

        // Add process and aligned log level (log.c:376-379): "P%0*u %*s: "
        let level_name = level.as_str();
        text.push_str(&format!(
            "P{process_id:0process_size$} {level_name:>LOG_LEVEL_WIDTH$}: "
        ));

        // When writing to stderr the timestamp, process, and log level alignment will be skipped
        // (log.c:381-382): rewind over ": " and the level name.
        let stderr_offset = text.len() - level_name.len() - 2;

        // Set the indent size (log.c:384-385) -- this will need to be adjusted for stderr
        let mut indent_size = text.len();

        // Add error code (log.c:387-389): "[%03d]: "
        if code != 0 {
            text.push_str(&format!("[{code:03}]: "));
        }

        // Add dry-run prefix (log.c:391-393)
        if dry_run {
            text.push_str(DRY_RUN_PREFIX);
        }

        // Add debug info (log.c:395-408)
        if let Some(padding) = debug_padding(level) {
            text.push_str(&" ".repeat(padding));
            // ... result.indentSize++ per padding byte (log.c:402)
            indent_size += padding;

            // "%.*s::%s: " with precision strlen(fileName) - 2 (log.c:405-407)
            let trimmed = file_name.len().saturating_sub(2);
            text.push_str(&format!(
                "{file_name}:{}{function_name}: ",
                &file_name[..trimmed]
            ));
        }

        Self {
            text,
            stderr_offset,
            indent_size,
        }
    }

    /// The full header, as the file and stdout columns get it.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The header as stderr sees it, i.e. from the level name on (`log.c:382`, `log.c:450`).
    #[must_use]
    pub fn stderr_text(&self) -> &str {
        &self.text[self.stderr_offset..]
    }

    /// `indentSize` (`log.c:385`), used for stdout and the log file.
    #[must_use]
    pub const fn indent_size(&self) -> usize {
        self.indent_size
    }

    /// The indent stderr uses: `indentSize - (logBufferStdErr - logBuffer)` (`log.c:451`).
    #[must_use]
    pub const fn stderr_indent_size(&self) -> usize {
        self.indent_size - self.stderr_offset
    }
}

/// A formatted log line, i.e. `logBuffer` after `logPost()` has finished it (`log.c:418-479`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// The full line, linefeed included, no NUL.
    text: String,
    /// Byte offset of [`Header::stderr_text`] inside [`Self::text`].
    stderr_offset: usize,
    /// `indentSize` for stdout/file continuation lines.
    indent_size: usize,
    /// stderr continuation indent.
    stderr_indent_size: usize,
}

impl Line {
    /// Assemble header + message the way `logInternal()` does (`log.c:530-536`), then finish the line
    /// the way `logPost()` does (`log.c:437-442`).
    #[must_use]
    pub fn new(header: &Header, message: &str) -> Self {
        let header_len = header.text().len();

        // Copy message into buffer and update buffer position (log.c:533-534). snprintf() writes at most
        // LOG_BUFFER_SIZE - bufferPos - 1 bytes but returns the *untruncated* length, and that return
        // value is what bufferPos advances by — the hazard upstream commit c6e7ba81a clamped in logPost().
        let written = message.len().min(LOG_BUFFER_SIZE - header_len - 1);
        let mut text = String::with_capacity(header_len + written + 1);
        text.push_str(header.text());
        text.push_str(&message[..truncate_boundary(message, written)]);

        // The position the linefeed lands at is the untruncated one, clamped to leave room for the
        // linefeed and the terminator (log.c:437-438)
        Self::from_buffer(header, text, header_len + message.len())
    }

    /// A `logSignal()` line (`log.c:484-508`): no header at all — the buffer is seeded with the fixed
    /// `"terminated on signal "` prefix, `logBufferStdErr` is `logBuffer` itself (so stderr loses nothing)
    /// and `indentSize` is hardcoded to 4.
    #[must_use]
    pub fn signal(signal_name: &str) -> Self {
        let header = Header {
            text: String::new(),
            stderr_offset: 0,
            indent_size: 4,
        };

        // memcpy() of the prefix, then the name at bufferPos (log.c:494-502)
        Self::from_buffer(
            &header,
            format!("{LOG_SIGNAL_MESSAGE_PRE}{signal_name}"),
            LOG_SIGNAL_MESSAGE_PRE.len() + signal_name.len(),
        )
    }

    /// `logPost()`'s tail (`log.c:435-442`): clamp the position, put a linefeed at it, terminate after.
    ///
    /// Because the linefeed is written *at* the position rather than appended, an over-long message loses
    /// its last byte: for a full buffer the emitted line is `LOG_BUFFER_SIZE - 1` bytes ending in `\n`,
    /// which is exactly what `logTest.c:212-213` asserts.
    fn from_buffer(header: &Header, mut text: String, mut buffer_pos: usize) -> Self {
        if buffer_pos > LOG_BUFFER_SIZE - 2 {
            buffer_pos = LOG_BUFFER_SIZE - 2;
        }

        text.truncate(truncate_boundary(&text, buffer_pos));
        text.push('\n');
        debug_assert!(text.len() <= LOG_BUFFER_SIZE);

        Self {
            text,
            stderr_offset: header.stderr_offset,
            indent_size: header.indent_size,
            stderr_indent_size: header.stderr_indent_size(),
        }
    }

    /// The full line, terminated by `\n`, exactly as it sits in `logBuffer`.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The line as stderr gets it: from the level name on, timestamp and process id dropped.
    #[must_use]
    pub fn stderr_text(&self) -> &str {
        &self.text[self.stderr_offset..]
    }

    /// Continuation indent for stdout and the log file.
    #[must_use]
    pub const fn indent_size(&self) -> usize {
        self.indent_size
    }

    /// Continuation indent for stderr.
    #[must_use]
    pub const fn stderr_indent_size(&self) -> usize {
        self.stderr_indent_size
    }

    /// The `indentSize` sanity bound `logWriteIndent()` asserts (`log.c:308`):
    /// `indentSize > 0 && indentSize < sizeof(indentBuffer)`, and the literal at `log.c:297` is 90
    /// spaces, so `sizeof` is 91.
    #[must_use]
    pub const fn indent_size_valid(size: usize) -> bool {
        size > 0 && size < 91
    }
}

/// `logWriteIndent()` (`log.c:293-329`): indent every line after the first.
///
/// The C writes `indentBuffer` before each line *except* the first and except after the last linefeed,
/// so the returned pieces are the indent-free first line, then the indented middle lines, and the tail
/// with no trailing indent. `join_lines()` reassembles what one `write()`-per-piece sequence produces.
#[must_use]
pub fn split_lines(text: &str, indent_size: usize) -> Vec<String> {
    let indent = " ".repeat(indent_size);
    let mut pieces = Vec::new();

    for (index, line) in text.split_inclusive('\n').enumerate() {
        if index == 0 {
            pieces.push(line.to_string());
        } else {
            pieces.push(format!("{indent}{line}"));
        }
    }

    pieces
}

/// Reassemble [`split_lines`] output, for tests that compare against a whole expected log file.
#[must_use]
pub fn join_lines(pieces: &[String]) -> String {
    pieces.concat()
}

/// Truncate to `len` bytes without splitting a UTF-8 character.
///
/// The C would split it; see the crate notes on why that only differs for non-ASCII messages.
fn truncate_boundary(text: &str, len: usize) -> usize {
    let mut end = len.min(text.len());

    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }

    end
}

/// The level names, in `logLevelList[]` order (`log.c:69-78`) — exposed so the tests can pin the table
/// against the C.
#[must_use]
pub fn level_names() -> [&'static str; 8] {
    [
        "OFF", "ASSERT", "ERROR", "WARN", "INFO", "DETAIL", "DEBUG", "TRACE",
    ]
}

/// The user-selectable level names, in `build/config.yaml` option order — i.e. `off, error, warn,
/// info, detail, debug, trace` of the ticket, which is [`LOG_LEVEL_SEQ`] spelled as strings.
#[must_use]
pub fn level_option_names() -> [&'static str; 7] {
    LOG_LEVEL_SEQ.map(|level| level.as_str())
}
