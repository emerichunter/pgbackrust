//! A [`Console`] that keeps bytes, so the byte-exact format and routing rules can be asserted.
//!
//! Upstream tests this by pointing `logFdStdOut`/`logFdStdErr`/`logFdFile` at temp files and reading them
//! back (`test/src/module/common/logTest.c:32-68` `testErrorFile`). Redirecting a real process's standard
//! descriptors from a Rust unit test is not workable — cargo owns them and tests run concurrently — so
//! every destination is a buffer here instead. The bytes are the same ones `logWrite()` would have handed
//! to `write()`.
//!
//! Each destination is also a separate buffer because upstream's routing decisions are per-fd: a test that
//! merged stdout into stderr could not tell "went to stderr only" from "went to both".

use std::io;
use std::sync::Mutex;

use crate::handler::{Console, Target};

/// One destination's byte log plus the file state `log.c:33-39` tracks.
#[derive(Default, Debug)]
pub struct CaptureConsole {
    stdout: Mutex<Vec<u8>>,
    stderr: Mutex<Vec<u8>>,
    file: Mutex<Vec<u8>>,
    file_open: Mutex<bool>,
    file_banner_written: Mutex<bool>,
    file_size: Mutex<u64>,
    /// When set, every `write()` fails with this, standing in for a full disk.
    fail_with: Mutex<Option<String>>,
}

impl CaptureConsole {
    /// A console with every destination empty and no file open.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// `logFileSet()` succeeded with a file of `size` bytes already in it — the state `log.c:465`'s
    /// `lseek()` looks at to decide whether the banner needs a blank line first.
    pub fn open_file_with_size(&self, size: u64) {
        *self.lock(&self.file_open) = true;
        *self.lock(&self.file_size) = size;
        *self.lock(&self.file_banner_written) = false;
    }

    /// Fail every subsequent write, as if the target were full. The string is the `io::Error` text.
    pub fn fail_writes(&self, message: &str) {
        *self.lock(&self.fail_with) = Some(message.to_string());
    }

    /// Stop failing writes.
    pub fn allow_writes(&self) {
        *self.lock(&self.fail_with) = None;
    }

    /// Bytes written to a destination, and reset it.
    #[must_use]
    pub fn take(&self, target: Target) -> String {
        let bytes = match target {
            Target::StdOut => std::mem::take(&mut *self.lock(&self.stdout)),
            Target::StdErr => std::mem::take(&mut *self.lock(&self.stderr)),
            Target::File => std::mem::take(&mut *self.lock(&self.file)),
        };

        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// Everything written to stdout so far.
    #[must_use]
    pub fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.lock(&self.stdout)).into_owned()
    }

    /// Everything written to stderr so far.
    #[must_use]
    pub fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.lock(&self.stderr)).into_owned()
    }

    /// Everything written to the log file so far, banner included.
    #[must_use]
    pub fn file(&self) -> String {
        String::from_utf8_lossy(&self.lock(&self.file)).into_owned()
    }

    fn lock<'a, T>(&self, value: &'a Mutex<T>) -> std::sync::MutexGuard<'a, T> {
        value
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Console for CaptureConsole {
    fn write(&self, target: Target, bytes: &[u8]) -> io::Result<()> {
        if let Some(message) = self.lock(&self.fail_with).as_ref() {
            return Err(io::Error::other(message.clone()));
        }

        match target {
            Target::StdOut => self.lock(&self.stdout).extend_from_slice(bytes),
            Target::StdErr => self.lock(&self.stderr).extend_from_slice(bytes),
            Target::File => self.lock(&self.file).extend_from_slice(bytes),
        }

        Ok(())
    }

    fn file_open(&self) -> bool {
        *self.lock(&self.file_open)
    }

    fn file_banner_written(&self) -> bool {
        *self.lock(&self.file_banner_written)
    }

    fn set_file_banner_written(&self, written: bool) {
        *self.lock(&self.file_banner_written) = written;
    }

    fn file_size(&self) -> io::Result<u64> {
        Ok(*self.lock(&self.file_size))
    }

    fn close_file(&self) -> io::Result<()> {
        *self.lock(&self.file_open) = false;
        *self.lock(&self.file_banner_written) = false;
        *self.lock(&self.file_size) = 0;

        Ok(())
    }

    fn open_file(&self, _path: &str) -> io::Result<()> {
        *self.lock(&self.file_open) = true;
        *self.lock(&self.file_size) = 0;
        *self.lock(&self.file_banner_written) = false;

        Ok(())
    }
}
