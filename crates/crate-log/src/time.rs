//! Clock and timestamp rendering for the log header, ported from pgBackRest
//! `src/common/time.c:timeMSec()` and `src/common/type/convert.c:cvtTimeToZ()`.
//!
//! `logPre()` (`src/common/log.c:365-374`) builds the timestamp column with two calls:
//!
//! ```c
//! result.bufferPos += cvtTimeToZP("%Y-%m-%d %H:%M:%S", logTimeSec, ...);
//! result.bufferPos += snprintf(..., ".%03d ", (int)(logTimeMSec % 1000));
//! ```
//!
//! `cvtTimeToZ()` runs `strftime()` over the `struct tm` from `localtime_r()` (`convert.c:566-573`), so
//! the column is local wall-clock time rather than UTC, and `%Y-%m-%d %H:%M:%S` is always exactly 19
//! bytes. The milliseconds add `.{SSS}` plus one space — 5 bytes — for a 24-byte column. This module
//! reproduces both halves byte for byte.
//!
//! # Local time without libc
//!
//! `std` has no local-time conversion, and calling `localtime_r()` would be an `unsafe` FFI call, which
//! AGENTS.md R7 confines to a designated crate. So the civil-calendar arithmetic is done in safe Rust
//! (Howard Hinnant's `civil_from_days`, the same proleptic Gregorian calendar `strftime()` uses) and
//! the UTC offset comes from [`set_local_offset_hook`].
//!
//! **Proposed deviation, pending user approval** (`docs/COMPATIBILITY_CONTRACT.md`): with no hook
//! installed the offset is UTC. Upstream would use the libc zone database, so in a non-UTC zone the
//! wall-clock stamp differs by the zone offset — the *format* is identical either way. The offset
//! belongs to the zone handling that ships with `src/common/time.c`'s own crate, not to `crate-log`;
//! installing the hook is how that crate will take over. Guessing at a TZif reader here would risk
//! silently wrong stamps, which is worse than an explicit, documented default.

use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// `MSEC_PER_SEC` (`src/common/time.h:19`).
pub const MSEC_PER_SEC: i64 = 1_000;

/// Milliseconds since the Unix epoch, i.e. `timeMSec()` (`src/common/time.c:19-27`).
///
/// Upstream reads `gettimeofday()`; `SystemTime::now()` is the same clock. Clamped to `i64::MAX` for an
/// implausibly distant future clock, since upstream's `long long` would simply overflow.
#[must_use]
pub fn time_msec() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(elapsed) => i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX),
        Err(before_epoch) => {
            -i64::try_from(before_epoch.duration().as_millis()).unwrap_or(i64::MIN)
        }
    }
}

/// Seconds a local wall clock is ahead of UTC, resolved from a UTC second.
type LocalOffsetHook = dyn Fn(i64) -> i32 + Send + Sync + 'static;

static LOCAL_OFFSET_HOOK: OnceLock<&'static LocalOffsetHook> = OnceLock::new();

/// Install the zone provider used for log timestamps.
///
/// Calling it twice keeps the first provider, the way `tzset()` caches the zone for the process.
pub fn set_local_offset_hook(hook: &'static LocalOffsetHook) {
    let _ = LOCAL_OFFSET_HOOK.set(hook);
}

/// The local offset for a UTC second, `0` (UTC) when no zone provider is installed.
#[must_use]
pub fn local_offset(unix_sec: i64) -> i32 {
    LOCAL_OFFSET_HOOK.get().map_or(0, |hook| (hook)(unix_sec))
}

/// A local date and time with no zone attached, i.e. what `strftime()` sees after `localtime_r()`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CivilDateTime {
    /// Calendar year, e.g. 2026.
    pub year: i64,
    /// Month, 1-12.
    pub month: u32,
    /// Day of month, 1-31.
    pub day: u32,
    /// Hour, 0-23.
    pub hour: u32,
    /// Minute, 0-59.
    pub minute: u32,
    /// Second, 0-59.
    pub second: u32,
    /// Whole milliseconds within the second, 0-999.
    pub msec: u32,
}

impl CivilDateTime {
    /// The `"%Y-%m-%d %H:%M:%S"` part of the stamp: 19 bytes, no trailing space.
    #[must_use]
    pub fn format_log_second(self) -> String {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }

    /// The complete 24-byte timestamp column: `format_log_second()` plus `".%03d "` (`log.c:372-373`).
    #[must_use]
    pub fn format_log_timestamp(self) -> String {
        format!("{}.{} ", self.format_log_second(), self.msec)
    }
}

/// Render a log timestamp column directly from milliseconds and a UTC offset.
///
/// Exposed so a byte-exact stamp can be asserted without a wall clock: `log.c` derives the column from
/// exactly these two values (`logTimeSec` and `logTimeMSec`).
#[must_use]
pub fn format_log_timestamp_with_offset(msec: i64, offset_secs: i32) -> String {
    date_time_with_offset(msec, offset_secs).format_log_timestamp()
}

/// Milliseconds since the epoch to local wall-clock time, using the installed zone (see [`local_offset`]).
#[must_use]
pub fn local_date_time(msec: i64) -> CivilDateTime {
    date_time_with_offset(msec, local_offset(msec.div_euclid(MSEC_PER_SEC)))
}

/// The current local stamp, i.e. `timeMSec()` through `localtime_r()` as `logPre()` does.
#[must_use]
pub fn now() -> CivilDateTime {
    local_date_time(time_msec())
}

/// Clock provider: returns the stamp a log line is given.
type ClockHook = dyn Fn() -> CivilDateTime + Send + Sync + 'static;

static CLOCK_HOOK: OnceLock<&'static ClockHook> = OnceLock::new();

/// Install the clock the process-wide logger stamps with.
///
/// `logPre()` calls `timeMSec()` per line (`log.c:367`); a hook is how a test asserts a byte-exact
/// timestamp without controlling the host clock. Calling it twice keeps the first provider.
pub fn set_clock(clock: &'static ClockHook) {
    let _ = CLOCK_HOOK.set(clock);
}

/// The stamp to use: the installed clock, else [`now`].
#[must_use]
pub fn timestamp() -> CivilDateTime {
    CLOCK_HOOK.get().map_or_else(now, |hook| (hook)())
}

/// Milliseconds since the epoch to wall-clock time at a fixed UTC offset.
///
/// # Negative times
///
/// Upstream computes `logTimeSec = logTimeMSec / MSEC_PER_SEC` and `(int)(logTimeMSec % 1000)`, and C
/// truncates toward zero, so for a pre-1970 clock the two parts disagree (`-1500` becomes second `-1`,
/// milli `-500` and prints `1969-12-31 23:59:59.-500 `). Flooring here keeps the second and the
/// millisecond field self-consistent (`1969-12-31 23:59:58.500 `). A pre-epoch clock is the only case
/// where the two differ, so a normal clock is byte-identical to upstream.
#[must_use]
pub fn date_time_with_offset(msec: i64, offset_secs: i32) -> CivilDateTime {
    let second = msec.div_euclid(MSEC_PER_SEC);
    let milli = msec.rem_euclid(MSEC_PER_SEC) as u32;
    let local = second + i64::from(offset_secs);
    let days = local.div_euclid(86_400);
    let second_of_day = local.rem_euclid(86_400) as u32;
    let (year, month, day) = civil_from_days(days);

    CivilDateTime {
        year,
        month,
        day,
        hour: second_of_day / 3_600,
        minute: second_of_day % 3_600 / 60,
        second: second_of_day % 60,
        msec: milli,
    }
}

/// Days since 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's `civil_from_days`).
#[must_use]
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_097 + 1
    } / 146_097;
    let day_of_era = shifted - era * 146_097; // [0, 146096]
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365; // [0, 399]
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100); // [0, 365]
    let shifted_month = (day_of_year + 51) / 153; // March-based month [0, 11]
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = shifted_month + if shifted_month < 10 { 3 } else { -9 };

    (
        if shifted_month < 10 { year + 1 } else { year },
        month as u32,
        day as u32,
    )
}

/// A proleptic Gregorian date to days since 1970-01-01 (inverse of [`civil_from_days`]).
#[must_use]
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400; // [0, 399]
    let month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * i64::from(month) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;

    era * 146_097 + day_of_era - 719_468
}
