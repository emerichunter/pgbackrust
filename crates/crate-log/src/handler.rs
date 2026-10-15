//! The log handler: state and writes, ported from pgBackRest `src/common/log.c:22-49` (statics),
//! `105-264` (`logAnySet`/`logAny`/`logFileClose`/`logFileSet`/`logInit`/`logClose`/`logRange`) and
//! `418-545` (`logPost`/`logSignal`/`logInternal`).
//!
//! Upstream reaches twelve process-wide statics from anywhere. A Rust port needs the same decisions to be
//! testable without taking over the real process streams, so this module is split in three:
//!
//! - [`route`] is `logPost()`'s destination logic as a pure function.
//! - [`Logger`] holds the state the C statics hold and writes through a [`Console`], so a test points it at
//!   buffers and compares bytes.
//! - [`init`], [`set_file`], [`log`], [`any`] and [`log_signal`] drive the one process-wide `Logger` over
//!   the real streams. That is what the `LOG_*` macros of `log.h` become.
//!
//! # Routing (`logPost()`, `log.c:444-476`)
//!
//! Three independent questions, in order:
//!
//! 1. `level <= logLevelStdErr`, and then the range test runs against **whichever console target is the
//!    noisier**: `logRange(logLevelStdErr, ..)` when `logLevelStdErr > logLevelStdOut`, else
//!    `logRange(logLevelStdOut, ..)`. Write the stderr view of the line.
//! 2. **otherwise** (`else if`, `log.c:455`) `level <= logLevelStdOut` and `logRange(logLevelStdOut, ..)`.
//!    Write the full line.
//! 3. separately — not part of the `if`/`else if` chain — `level <= logLevelFile` and `logFdFile != -1` and
//!    `logRange(logLevelFile, ..)`. Write the full line, after the banner on the first write.
//!
//! So a line can go to stderr *and* the file, or stdout *and* the file, but never to stdout *and* stderr.
//! Step 1 gates on the message's level but range-tests a threshold rather than the level: that asymmetry is
//! what `logTest.c:266-290` pins ("error goes to stderr when stderr is not in range but stdout is", and the
//! reverse), and it is reproduced exactly.
//!
//! # Writes are one `write()` per line (`logWriteIndent()`, `log.c:293-329`)
//!
//! Upstream issues a separate `write()` per piece: the first line unindented, then indent + line for every
//! following line, and no indent after the final linefeed. `logBuffer` itself keeps its interior newlines
//! (`logTest.c:228` expects `"P44  ERROR: [026]: message1\nmessage2\n"` as one buffer) — only the emitted
//! bytes are re-split, which is why a log file's last line is not followed by an indent run.
//! [`crate::format::split_lines`] is the shared piece list.
//!
//! # Errors
//!
//! `logWrite()` (`log.c:266-288`) throws `file-write` (64) as `unable to write {detail}` when `write()`
//! returns short, where `detail` is `"log to stdout"`, `"log to stderr"`, `"log to file"`, `"banner to
//! file"` or `"banner spacing to file"`. [`write_error`] renders the same message from an `io::Error`, and
//! [`Logger::log`] hands it back — `logInternal()` "may throw" in upstream terms.
//!
//! # Asserts
//!
//! Upstream's `ASSERT()` compiles away outside a `DEBUG` build (`src/common/assert.h:13`), and the ones in
//! `logPre()`/`logPost()`/`logWriteIndent()` check values the handler computed itself. They are ported as
//! `debug_assert!()`, omitted from a release build for the same reason. The asserts that a caller can trip
//! and that upstream tests — `logLevelEnum()`'s bound (`log.c:88`) and `logInit()`'s `processId <= 999`
//! (`log.c:155-156`) — are real `assert` errors, so the ported behavior is observable.
//!
//! # Assumptions
//!
//! - **Destinations are a [`Console`] trait, not fd numbers.** Upstream `write()`s to `STDOUT_FILENO`,
//!   `STDERR_FILENO` and one `open()`ed descriptor. `std::io` owns those streams and `write_all()` is the
//!   short-write check `logWrite()` hand-rolls, so the trait keeps the error text and code identical and
//!   lets the parity tests capture bytes.
//! - **State is behind a `Mutex`, not plain statics.** Upstream's statics are single-threaded by
//!   construction; PgBackRust uses threads for local parallelism (AGENTS.md section 3), so the equivalent
//!   state needs a lock. Upstream's single static `logBuffer` is deliberately not shared: each call formats
//!   its own line, so two threads cannot interleave headers the way they would corrupt the C buffer.
//! - **`lseek(logFdFile, 0, SEEK_END) > 0`** (`log.c:465`) decides whether a blank line precedes the banner.
//!   The file is opened `O_APPEND`, so its length on open is the same answer without moving the offset.
//! - **Log file mode `0640`** (`log.c:210`) is applied on unix only. File permissions are an R5 surface, and
//!   this mirrors upstream, whose `open()` flags are unix-specific too.
//! - **A failed `logFileSet()` open** (`log.c:210-216`) warns and returns `false`. The `WARN` message text is
//!   reproduced; the `strerror()` half carries Rust's `io::Error` rendering, which embeds the same libc
//!   string plus a ` (os error N)` suffix — the same pending deviation recorded for `crate-error`.
//! - **`logFileSet()`'s double `logAnySet()`** (`log.c:223` and `log.c:226`) is dead in C: the first call is
//!   immediately overwritten by the second, which always runs. Here `any` is derived from the config on every
//!   call, so there is nothing to cache and no equivalent of the redundancy to port.

use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::sync::{Mutex, OnceLock};

use crate::format::{Header, LOG_BANNER, Line, split_lines};
use crate::level::{LOG_PROCESS_ID_NONE, LogLevel};
use crate::time;
use crate_error::{Error, error_type};

/// The destinations [`route`] may select, as flags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Route {
    /// `log.c:450` — the stderr view, indented by the stderr indent.
    pub stderr: bool,
    /// `log.c:455-456` — the full line, indented by `indentSize`.
    pub stdout: bool,
    /// `log.c:459-475` — the full line, after the banner.
    pub file: bool,
}

impl Route {
    /// Nothing selected: the message is dropped.
    #[must_use]
    pub const fn is_none(self) -> bool {
        !self.stderr && !self.stdout && !self.file
    }
}

/// `logPost()`'s destination decisions, including its three `logRange()` calls (`log.c:249-264`).
///
/// `file_open` stands in for `logFdFile != -1`. `min`/`max` are the range the caller handed
/// `logInternal()`; every `LOG_*` macro passes `LOG_LEVEL_MIN`..`LOG_LEVEL_MAX`, and `logSignal()` does
/// too (`log.c:505`), so in practice the range test only ever rejects on the thresholds themselves.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn route(
    level: LogLevel,
    stdout_level: LogLevel,
    stderr_level: LogLevel,
    file_level: LogLevel,
    file_open: bool,
    min: LogLevel,
    max: LogLevel,
) -> Route {
    debug_assert!(
        level >= LogLevel::Assert && level <= LogLevel::Trace,
        "logPost() ASSERT_LOG_LEVEL(logLevel) (log.c:432)"
    );
    debug_assert!(
        min <= max,
        "logPost() ASSERT(logRangeMin <= logRangeMax) (log.c:435)"
    );

    let mut result = Route::default();

    // Determine where to log the message based on log-level-stderr (log.c:444-454)
    if level <= stderr_level {
        result.stderr = if stderr_level > stdout_level {
            stderr_level.in_range(min, max)
        } else {
            stdout_level.in_range(min, max)
        };
    }
    // else if (log.c:455-456)
    else if level <= stdout_level && stdout_level.in_range(min, max) {
        result.stdout = true;
    }

    // Log to file (log.c:458-476) -- an independent if, so not exclusive with the console above
    result.file = level <= file_level && file_open && file_level.in_range(min, max);

    result
}

/// One of `logPost()`'s three destinations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// `logFdStdOut` (`log.c:31`).
    StdOut,
    /// `logFdStdErr` (`log.c:32`).
    StdErr,
    /// `logFdFile` (`log.c:33`).
    File,
}

/// Everything `logPost()` writes to, plus the log-file state `log.c:33-39` keeps.
pub trait Console: Send + Sync {
    /// `logWrite(fd, message, messageSize, errorDetail)` (`log.c:266-288`).
    fn write(&self, target: Target, bytes: &[u8]) -> io::Result<()>;

    /// `logFdFile != -1`.
    fn file_open(&self) -> bool;

    /// `logFileBanner` (`log.c:36`).
    fn file_banner_written(&self) -> bool;

    /// `logFileBanner = true` (`log.c:472`).
    fn set_file_banner_written(&self, written: bool);

    /// `lseek(logFdFile, 0, SEEK_END)` (`log.c:465`).
    fn file_size(&self) -> io::Result<u64>;

    /// `logFileClose()` (`log.c:175-187`).
    fn close_file(&self) -> io::Result<()>;

    /// `open(logFile, O_CREAT | O_APPEND | O_WRONLY, 0640)` (`log.c:210`).
    fn open_file(&self, path: &str) -> io::Result<()>;
}

/// `logWrite()`'s throw (`log.c:285`) from an `io::Error`: `file-write` (64), `unable to write {detail}`.
#[must_use]
pub fn write_error(detail: &str, error: io::Error) -> Error {
    Error::throw(
        error_type("file-write"),
        format!("unable to write {detail}: {error}"),
    )
}

/// The state `log.c:23-49` keeps in statics.
///
/// [`Default`] is upstream's initializer set exactly: stdout and stderr `logLevelError`, file
/// `logLevelOff`, `logLevelAny` `logLevelError`, no timestamp, process id 0, `logProcessSize` 2, no dry run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// `logLevelStdOut` (`log.c:25`).
    pub stdout: LogLevel,
    /// `logLevelStdErr` (`log.c:26`).
    pub stderr: LogLevel,
    /// `logLevelFile` (`log.c:27`).
    pub file: LogLevel,
    /// `logTimestamp` (`log.c:39`).
    pub timestamp: bool,
    /// `logProcessId` (`log.c:42`): the id used when a call passes [`LOG_PROCESS_ID_NONE`].
    pub process_id: u32,
    /// `logProcessSize` (`log.c:45`) — `logProcessMax > 99 ? 3 : 2` (`log.c:163`).
    pub process_size: usize,
    /// `logDryRun` (`log.c:48`).
    pub dry_run: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            stdout: LogLevel::Error,
            stderr: LogLevel::Error,
            file: LogLevel::Off,
            timestamp: false,
            process_id: 0,
            process_size: 2,
            dry_run: false,
        }
    }
}

impl Config {
    /// `logInit()` (`log.c:136-171`).
    ///
    /// # Errors
    /// Upstream `ASSERT`s `processId <= 999` and `logProcessMax <= 999` (`log.c:155-156`) — a caller can trip
    /// these and `logTest.c` does not cover them, so they become the `assert` error (25, fatal) with the C's
    /// message text.
    pub fn init(
        stdout: LogLevel,
        stderr: LogLevel,
        file: LogLevel,
        timestamp: bool,
        process_id: u32,
        process_max: u32,
        dry_run: bool,
    ) -> Result<Self, Error> {
        for (name, value) in [("processId", process_id), ("logProcessMax", process_max)] {
            if value > 999 {
                return Err(Error::at(
                    error_type("assert"),
                    format!("assertion '{name} <= 999' failed"),
                    file!(),
                    "logInit",
                    line!(),
                ));
            }
        }

        Ok(Self {
            stdout,
            stderr,
            file,
            timestamp,
            process_id,
            process_size: if process_max > 99 { 3 } else { 2 },
            dry_run,
        })
    }

    /// `logAnySet()` folded into `logAny()` (`log.c:109-134`): would a message at `level` be written
    /// anywhere? `logLevelAny` is the max of the three thresholds, with the file threshold counted only
    /// while a file is open.
    #[must_use]
    pub const fn any(&self, level: LogLevel, file_open: bool) -> bool {
        let mut any = self.stdout;

        if self.stderr.seq() > any.seq() {
            any = self.stderr;
        }

        if self.file.seq() > any.seq() && file_open {
            any = self.file;
        }

        level.seq() <= any.seq()
    }
}

/// The handler: configuration plus destinations.
#[derive(Debug)]
pub struct Logger<C: Console> {
    config: Mutex<Config>,
    console: C,
}

impl<C: Console> Logger<C> {
    /// A logger with the given configuration and console.
    #[must_use]
    pub fn new(config: Config, console: C) -> Self {
        Self {
            config: Mutex::new(config),
            console,
        }
    }

    /// Borrow the console, so a test can read what was written.
    #[must_use]
    pub fn console(&self) -> &C {
        &self.console
    }

    /// The current configuration, copied out of the lock.
    #[must_use]
    pub fn config(&self) -> Config {
        *self
            .config
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// `logInit()` (`log.c:136-171`).
    pub fn set_config(&self, config: Config) {
        *self
            .config
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = config;
    }

    /// `logClose()` (`log.c:236-243`): disable everything and close the file.
    ///
    /// # Errors
    /// `file-write` from the close path.
    pub fn close(&self) -> Result<(), Error> {
        // logInit(logLevelOff, logLevelOff, logLevelOff, false, 0, 1, false) (log.c:239)
        self.set_config(
            Config::init(
                LogLevel::Off,
                LogLevel::Off,
                LogLevel::Off,
                false,
                0,
                1,
                false,
            )
            .expect("logClose() passes values inside logInit()'s asserts"),
        );

        self.console
            .close_file()
            .map_err(|error| write_error("log file close", error))
    }

    /// `logAny()` (`log.c:125-134`).
    #[must_use]
    pub fn any(&self, level: LogLevel) -> bool {
        self.config().any(level, self.console.file_open())
    }

    /// `logFileSet()` (`log.c:190-227`): close any open file, then open `path` unless the file level is
    /// `off`. `Ok(false)` is upstream's `result = false` for a failed open, after the `WARN`.
    ///
    /// # Errors
    /// `file-write` when the warning about a failed open cannot itself be written.
    pub fn set_file(&self, path: Option<&str>) -> Result<bool, Error> {
        // Close the log file if it is already open (log.c:198)
        self.console
            .close_file()
            .map_err(|error| write_error("log file close", error))?;

        let config = self.config();

        // Only open the file if there is a chance to log something (log.c:207)
        let Some(path) = path else {
            return Ok(true);
        };

        if config.file == LogLevel::Off {
            return Ok(true);
        }

        match self.console.open_file(path) {
            // Output the banner on first log message (log.c:221)
            Ok(()) => Ok(true),
            Err(error) => {
                // LOG_WARN_FMT(...) (log.c:213-215)
                self.log(
                    LogLevel::Warn,
                    LOG_PROCESS_ID_NONE,
                    0,
                    "common/log.c",
                    "logFileSet",
                    &format!(
                        "unable to open log file '{path}': {error}\nNOTE: process will continue without log file."
                    ),
                )?;

                Ok(false)
            }
        }
    }

    /// The line `logInternal()` would put in `logBuffer`, without writing it.
    ///
    /// `process_id` is the `LOG_*` macro's `UINT_MAX` ([`LOG_PROCESS_ID_NONE`]) for "use the default".
    #[must_use]
    pub fn format_line(
        &self,
        level: LogLevel,
        process_id: u32,
        code: i32,
        file: &str,
        function: &str,
        message: &str,
    ) -> Line {
        let config = self.config();
        let header = self.header(level, process_id, code, file, function, &config);

        Line::new(&header, message)
    }

    fn header(
        &self,
        level: LogLevel,
        process_id: u32,
        code: i32,
        file: &str,
        function: &str,
        config: &Config,
    ) -> Header {
        Header::format(
            level,
            if process_id == LOG_PROCESS_ID_NONE {
                config.process_id
            } else {
                process_id
            },
            config.process_size,
            file,
            function,
            code,
            config.timestamp.then(time::timestamp),
            config.dry_run,
        )
    }

    /// `logInternal()` (`log.c:525-545`): `logPre()`, append the message, `logPost()`.
    ///
    /// # Errors
    /// `file-write` when a destination rejects the bytes.
    pub fn log(
        &self,
        level: LogLevel,
        process_id: u32,
        code: i32,
        file: &str,
        function: &str,
        message: &str,
    ) -> Result<(), Error> {
        let config = self.config();

        // logAny() short-circuit: the LOG_* macros skip logInternal() entirely (log.h:64-71)
        if !config.any(level, self.console.file_open()) {
            return Ok(());
        }

        let header = self.header(level, process_id, code, file, function, &config);

        self.emit(
            &Line::new(&header, message),
            self.route_for(level, &config, LogLevel::Assert, LogLevel::Trace),
        )
    }

    /// `logSignal()` (`log.c:484-508`): a hand-seeded buffer, no `logPre()`.
    ///
    /// # Errors
    /// `file-write` when a destination rejects the bytes.
    pub fn log_signal(&self, level: LogLevel, signal_name: &str) -> Result<(), Error> {
        let config = self.config();

        self.emit(
            &Line::signal(signal_name),
            self.route_for(level, &config, LogLevel::Assert, LogLevel::Trace),
        )
    }

    fn route_for(&self, level: LogLevel, config: &Config, min: LogLevel, max: LogLevel) -> Route {
        route(
            level,
            config.stdout,
            config.stderr,
            config.file,
            self.console.file_open(),
            min,
            max,
        )
    }

    /// `logPost()` (`log.c:418-479`), given the already-decided [`Route`].
    ///
    /// The linefeed is already in the line: [`Line`] finished it the way `logPost()` does, so what is left
    /// here is the destination selection and the indent split.
    fn emit(&self, line: &Line, route: Route) -> Result<(), Error> {
        // Determine where to log the message based on log-level-stderr (log.c:444-456)
        if route.stderr {
            self.write_indent(
                line.stderr_text(),
                line.stderr_indent_size(),
                "log to stderr",
                Target::StdErr,
            )?;
        } else if route.stdout {
            self.write_indent(
                line.text(),
                line.indent_size(),
                "log to stdout",
                Target::StdOut,
            )?;
        }

        // Log to file (log.c:458-476)
        if route.file {
            self.banner_once()?;
            self.write_indent(line.text(), line.indent_size(), "log to file", Target::File)?;
        }

        Ok(())
    }

    /// `logWriteIndent()` (`log.c:293-329`).
    fn write_indent(
        &self,
        text: &str,
        indent_size: usize,
        detail: &str,
        target: Target,
    ) -> Result<(), Error> {
        debug_assert!(
            Line::indent_size_valid(indent_size),
            "logWriteIndent() ASSERT(indentSize > 0 && indentSize < sizeof(indentBuffer)) (log.c:308)"
        );

        for piece in split_lines(text, indent_size) {
            self.console
                .write(target, piece.as_bytes())
                .map_err(|error| write_error(detail, error))?;
        }

        Ok(())
    }

    /// `log.c:461-473`: the banner once per file, with a blank line first if the file has content.
    fn banner_once(&self) -> Result<(), Error> {
        if self.console.file_banner_written() {
            return Ok(());
        }

        // Add a blank line if the file already has content (log.c:464-466)
        if self.console.file_size().unwrap_or(0) > 0 {
            self.console
                .write(Target::File, b"\n")
                .map_err(|error| write_error("banner spacing to file", error))?;
        }

        // Write process start banner (log.c:469) -- sizeof(LOG_BANNER) - 1, i.e. without the NUL
        self.console
            .write(Target::File, LOG_BANNER.as_bytes())
            .map_err(|error| write_error("banner to file", error))?;

        // Mark banner as written (log.c:471-472)
        self.console.set_file_banner_written(true);

        Ok(())
    }
}

/// The real destinations: stdout, stderr and one log file (`log.c:31-36`).
#[derive(Debug)]
pub struct ProcessConsole {
    file: Mutex<Option<BufWriter<File>>>,
    banner_written: Mutex<bool>,
    size: Mutex<u64>,
}

impl Default for ProcessConsole {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessConsole {
    /// A console writing to the real process streams, with no log file open.
    #[must_use]
    pub fn new() -> Self {
        Self {
            file: Mutex::new(None),
            banner_written: Mutex::new(false),
            size: Mutex::new(0),
        }
    }
}

impl Console for ProcessConsole {
    fn write(&self, target: Target, bytes: &[u8]) -> io::Result<()> {
        match target {
            Target::StdOut => io::stdout().lock().write_all(bytes),
            // C's stderr is unbuffered, and so is Rust's
            Target::StdErr => io::stderr().lock().write_all(bytes),
            Target::File => {
                let mut guard = lock(&self.file);
                let writer = guard.as_mut().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::NotConnected, "log file not open")
                })?;

                // Upstream write()s to the descriptor directly, so nothing may sit in a buffer
                writer.write_all(bytes)?;
                writer.flush()
            }
        }
    }

    fn file_open(&self) -> bool {
        lock(&self.file).is_some()
    }

    fn file_banner_written(&self) -> bool {
        *lock(&self.banner_written)
    }

    fn set_file_banner_written(&self, written: bool) {
        *lock(&self.banner_written) = written;
    }

    fn file_size(&self) -> io::Result<u64> {
        Ok(*lock(&self.size))
    }

    fn close_file(&self) -> io::Result<()> {
        if let Some(mut file) = lock(&self.file).take() {
            file.flush()?;
        }

        *lock(&self.banner_written) = false;
        *lock(&self.size) = 0;

        Ok(())
    }

    fn open_file(&self, path: &str) -> io::Result<()> {
        let mut options = OpenOptions::new();
        // O_CREAT | O_APPEND | O_WRONLY, 0640 (log.c:210)
        options.create(true).append(true).write(true);

        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o640);

        let file = options.open(path)?;
        let size = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
        *lock(&self.file) = Some(BufWriter::new(file));
        *lock(&self.size) = size;

        // logFileBanner = false (log.c:221)
        *lock(&self.banner_written) = false;

        Ok(())
    }
}

/// Lock a mutex, keeping the C static's behavior of never failing on a poison.
fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The process-wide handler, i.e. `log.c`'s statics plus [`ProcessConsole`].
static PROCESS: OnceLock<Logger<ProcessConsole>> = OnceLock::new();

/// The process-wide handler, created with upstream's default statics (`log.c:25-49`).
#[must_use]
pub fn process() -> &'static Logger<ProcessConsole> {
    PROCESS.get_or_init(|| Logger::new(Config::default(), ProcessConsole::new()))
}

/// `logInit()` for the process-wide handler.
///
/// # Errors
/// `assert` (25) when a process id exceeds upstream's 999 limit.
#[allow(clippy::too_many_arguments)]
pub fn init(
    stdout: LogLevel,
    stderr: LogLevel,
    file: LogLevel,
    timestamp: bool,
    process_id: u32,
    process_max: u32,
    dry_run: bool,
) -> Result<(), Error> {
    process().set_config(Config::init(
        stdout,
        stderr,
        file,
        timestamp,
        process_id,
        process_max,
        dry_run,
    )?);

    Ok(())
}

/// `logFileSet()` for the process-wide handler.
///
/// # Errors
/// `file-write` when the warning about a failed open cannot be written.
pub fn set_file(path: &str) -> Result<bool, Error> {
    process().set_file(Some(path))
}

/// `logFileClose()` for the process-wide handler.
///
/// # Errors
/// `file-write` from the close path.
pub fn close_file() -> Result<(), Error> {
    process().set_file(None).map(|_| ())
}

/// `logClose()` for the process-wide handler.
///
/// # Errors
/// `file-write` from the close path.
pub fn close() -> Result<(), Error> {
    process().close()
}

/// `logAny()` for the process-wide handler.
#[must_use]
pub fn any(level: LogLevel) -> bool {
    process().any(level)
}

/// `logInternal()` for the process-wide handler: the `LOG_*` macros of `log.h:118-135` become calls here,
/// passing [`LOG_PROCESS_ID_NONE`] for the default process id plus `__FILE__`/`__func__`.
///
/// # Errors
/// `file-write` when a destination rejects the bytes.
pub fn log(
    level: LogLevel,
    code: i32,
    file: &str,
    function: &str,
    message: &str,
) -> Result<(), Error> {
    process().log(level, LOG_PROCESS_ID_NONE, code, file, function, message)
}

/// `logSignal()` for the process-wide handler.
///
/// # Errors
/// `file-write` when a destination rejects the bytes.
pub fn log_signal(level: LogLevel, signal_name: &str) -> Result<(), Error> {
    process().log_signal(level, signal_name)
}
