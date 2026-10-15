//! Byte-exact log format tests. Every string asserted here is derived from logPre()/logPost()
//! in upstream src/common/log.c (the format table is documented in crate-log/src/format.rs).

use crate_log::capture::CaptureConsole;
use crate_log::format::{Header, LOG_LEVEL_WIDTH, Line, join_lines, level_names, split_lines};
use crate_log::handler::{Config, Logger, route};
use crate_log::level::{LOG_LEVEL_MAX, LOG_LEVEL_SEQ, LOG_PROCESS_ID_NONE, LogLevel};

#[test]
fn info_header_byte_exact() {
    // logPre() with no timestamp, pid 44 (width 2), level INFO, no code, no dry run.
    let header = Header::format(
        LogLevel::Info,
        44,
        2,
        "log.c",
        "logInternal",
        0,
        None,
        false,
    );

    // "P%0*u %*s: " with %*s = 6: the level field carries its own padding, so INFO gets 2 spaces.
    assert_eq!(header.text(), "P44   INFO: ");
    // stderr rewinds over ": " + the level name (log.c:381-382), losing pid and padding.
    assert_eq!(header.stderr_text(), "INFO: ");
    assert_eq!(header.indent_size(), 12);
    // stderr indent = indentSize - rewind distance (log.c:451).
    assert_eq!(header.stderr_indent_size(), 6);
}

#[test]
fn error_header_with_code_byte_exact() {
    let header = Header::format(
        LogLevel::Error,
        1,
        2,
        "log.c",
        "logInternal",
        26,
        None,
        false,
    );

    assert_eq!(header.text(), "P01  ERROR: [026]: ");
    assert_eq!(header.stderr_text(), "ERROR: [026]: ");
}

#[test]
fn line_appends_message_and_newline() {
    let header = Header::format(
        LogLevel::Error,
        1,
        2,
        "log.c",
        "logInternal",
        26,
        None,
        false,
    );
    let line = Line::new(&header, "boom");

    assert_eq!(line.text(), "P01  ERROR: [026]: boom\n");
    assert_eq!(line.stderr_text(), "ERROR: [026]: boom\n");
    assert_eq!(line.indent_size(), header.indent_size());
}

#[test]
fn wide_process_id_uses_width_3() {
    // logInit(logProcessMax > 99) -> processSize 3 (log.c:163).
    let header = Header::format(
        LogLevel::Info,
        123,
        3,
        "log.c",
        "logInternal",
        0,
        None,
        false,
    );
    assert_eq!(header.text(), "P123   INFO: ");
}

#[test]
fn dry_run_prefix_is_in_header_after_code() {
    let header = Header::format(LogLevel::Warn, 4, 2, "log.c", "logInternal", 0, None, true);
    assert_eq!(header.text(), "P04   WARN: [DRY-RUN] ");
}

#[test]
fn level_names_fit_the_column_width() {
    // The C hardcodes width 6 (log.c:379); a longer name would underflow the stderr rewind.
    for name in level_names() {
        assert!(
            name.len() <= LOG_LEVEL_WIDTH,
            "level '{name}' exceeds width {LOG_LEVEL_WIDTH}"
        );
    }
}

#[test]
fn from_seq_matches_upstream_table_order() {
    assert_eq!(LogLevel::from_seq(0).expect("off"), LogLevel::Off);
    assert_eq!(LogLevel::from_seq(1).expect("error"), LogLevel::Error);
    assert_eq!(LogLevel::from_seq(2).expect("warn"), LogLevel::Warn);
    assert_eq!(LogLevel::from_seq(3).expect("info"), LogLevel::Info);
    assert_eq!(LogLevel::from_seq(4).expect("detail"), LogLevel::Detail);
    assert_eq!(LogLevel::from_seq(5).expect("debug"), LogLevel::Debug);
    assert_eq!(LogLevel::from_seq(6).expect("trace"), LogLevel::Trace);

    // seq >= LOG_LEVEL_MAX (7) becomes the upstream assert error.
    let error = LogLevel::from_seq(LOG_LEVEL_MAX.seq()).expect_err("out of range must fail");
    assert_eq!(error.code(), 25);
    assert!(error.message().contains("logLevelSeq < LOG_LEVEL_MAX"));
}

#[test]
fn seq_table_matches_config_value_list() {
    // build/config.yaml lists off, error, warn, info, detail, debug, trace (logLevelEnum skip).
    let names: Vec<&str> = LOG_LEVEL_SEQ.iter().map(|level| level.as_str()).collect();
    assert_eq!(
        names,
        ["OFF", "ERROR", "WARN", "INFO", "DETAIL", "DEBUG", "TRACE"]
    );
}

#[test]
fn route_never_selects_stdout_and_stderr_together() {
    for stdout in LOG_LEVEL_SEQ {
        for stderr in LOG_LEVEL_SEQ {
            let result = route(
                LogLevel::Error,
                stdout,
                stderr,
                LogLevel::Off,
                false,
                LogLevel::Assert,
                LogLevel::Trace,
            );
            assert!(
                !(result.stdout && result.stderr),
                "stdout+stderr collision at stdout={stdout:?} stderr={stderr:?}"
            );
        }
    }
}

#[test]
fn split_lines_indents_continuations_only() {
    let pieces = split_lines("P01 ERROR: one\ntwo\nthree\n", 4);
    assert_eq!(join_lines(&pieces), "P01 ERROR: one\n    two\n    three\n");
}

#[test]
fn capture_console_byte_exact_end_to_end() {
    let console = CaptureConsole::new();
    let logger = Logger::new(Config::default(), console);

    logger
        .log(
            LogLevel::Error,
            LOG_PROCESS_ID_NONE,
            26,
            "test.c",
            "test_fn",
            "boom",
        )
        .expect("log succeeds");

    // Default config: stdout=stderr=Error, file=Off, pid 0. Route picks stderr (never both).
    let console = logger.console();
    assert_eq!(console.stderr(), "ERROR: [026]: boom\n");
    assert_eq!(console.stdout(), "");
}

#[test]
fn signal_line_has_fixed_prefix() {
    let line = Line::signal("SIGTERM");
    assert_eq!(line.text(), "terminated on signal SIGTERM\n");
    assert_eq!(line.stderr_text(), "terminated on signal SIGTERM\n");
    assert_eq!(line.indent_size(), 4);
}
