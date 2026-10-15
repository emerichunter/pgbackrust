//! Log levels ported from pgBackRest `src/common/logLevel.h` and `src/common/log.c:58-99`.

use crate_error::{Error, error_type};

/// Log levels, with the upstream numeric values from `src/common/logLevel.h:9-18`.
///
/// `Assert` sits between `Off` and `Error` and is never user-selectable: the value list of the
/// `log-level-*` options (`build/config.yaml`) is `off, error, warn, info, detail, debug, trace`, and
/// `logLevelEnum()` (`log.c:83-88`) skips `Assert` by adding one to every non-zero sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u32)]
pub enum LogLevel {
    /// `logLevelOff` = 0.
    Off = 0,
    /// `logLevelAssert` = 1. Internal: only an `ASSERT()` raise of `AssertError` uses it.
    Assert = 1,
    /// `logLevelError` = 2.
    Error = 2,
    /// `logLevelWarn` = 3.
    Warn = 3,
    /// `logLevelInfo` = 4.
    Info = 4,
    /// `logLevelDetail` = 5.
    Detail = 5,
    /// `logLevelDebug` = 6.
    Debug = 6,
    /// `logLevelTrace` = 7.
    Trace = 7,
}

/// `LOG_LEVEL_MIN` (`logLevel.h:21`), i.e. `logLevelAssert`.
pub const LOG_LEVEL_MIN: LogLevel = LogLevel::Assert;

/// `LOG_LEVEL_MAX` (`logLevel.h:22`), i.e. `logLevelTrace`.
pub const LOG_LEVEL_MAX: LogLevel = LogLevel::Trace;

/// `UINT_MAX` passed by the `LOG_*` macros in place of a process id (`log.h:130-131`), which means
/// "use the process id given to `logInit()`" (`log.c:378-379`).
pub const LOG_PROCESS_ID_NONE: u32 = u32::MAX;

/// The user-selectable levels in upstream option order. The index into this table is the sequence
/// accepted by [`LogLevel::from_seq`] — i.e. the value list of the `log-level-*` options.
pub const LOG_LEVEL_SEQ: [LogLevel; 7] = [
    LogLevel::Off,
    LogLevel::Error,
    LogLevel::Warn,
    LogLevel::Info,
    LogLevel::Detail,
    LogLevel::Debug,
    LogLevel::Trace,
];

impl LogLevel {
    /// `logLevelStr()` (`log.c:90-99`) returning `logLevelList[]` (`log.c:69-78`).
    ///
    /// Upstream `assert()`s `logLevel <= LOG_LEVEL_MAX`; the enum cannot hold a larger value, so that
    /// assert is enforced by the type instead.
    ///
    /// The strings are the byte-exact level column of a log line, and every one of them is at most 6
    /// characters wide — the width the header pads the level to. `format::LOG_LEVEL_WIDTH` is asserted
    /// against these values in the crate's tests because upstream leaves that coupling unchecked.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::Assert => "ASSERT",
            Self::Error => "ERROR",
            Self::Warn => "WARN",
            Self::Info => "INFO",
            Self::Detail => "DETAIL",
            Self::Debug => "DEBUG",
            Self::Trace => "TRACE",
        }
    }

    /// The upstream numeric value.
    #[must_use]
    pub const fn seq(self) -> u32 {
        self as u32
    }

    /// `logLevelEnum(logLevelSeq)` (`log.c:83-88`).
    ///
    /// # Errors
    /// Upstream `ASSERT(logLevelSeq < LOG_LEVEL_MAX)` becomes an `assert` error (code 25, fatal) whose
    /// message is byte-identical to the C: `assertion 'logLevelSeq < LOG_LEVEL_MAX' failed`.
    pub fn from_seq(seq: u32) -> Result<Self, Error> {
        if seq >= LOG_LEVEL_MAX.seq() {
            return Err(Error::at(
                error_type("assert"),
                "assertion 'logLevelSeq < LOG_LEVEL_MAX' failed",
                file!(),
                "logLevelEnum",
                line!(),
            ));
        }

        // Indexing the option-order table is the C `logLevelSeq += logLevelSeq > 0` (log.c:90).
        // The bounds guard above guarantees `seq < LOG_LEVEL_MAX.seq()` (7), so the index is safe.
        Ok(LOG_LEVEL_SEQ[seq as usize])
    }

    /// Parse a level name the way the config layer does (case-insensitive, like upstream `strId`), for
    /// the `log-level-*` options.
    ///
    /// # Errors
    /// The `config` error (code 27) when `name` is not one of the option values, matching the shape of
    /// the message the upstream option validator produces for a bad value.
    ///
    /// # Assumption
    /// `log.c` itself only converts sequence numbers and levels to strings; rejecting an unknown name
    /// happens in the config layer (`build/config.yaml` value list plus `option.c`), which is not
    /// ported yet. The error type and text here are therefore provisional and will be replaced by the
    /// real validator message when `crate-config` lands.
    pub fn from_name(name: &str) -> Result<Self, Error> {
        LOG_LEVEL_SEQ
            .iter()
            .copied()
            .find(|level| level.as_str().eq_ignore_ascii_case(name))
            .ok_or_else(|| {
                Error::throw(
                    error_type("config"),
                    format!("log level '{name}' not allowed"),
                )
            })
    }

    /// `logRange()` (`log.c:249-264`): is the level inside the inclusive range?
    #[must_use]
    pub const fn in_range(self, min: LogLevel, max: LogLevel) -> bool {
        self.seq() >= min.seq() && self.seq() <= max.seq()
    }
}
