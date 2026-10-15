//! Byte buffer ported from pgBackRest `src/common/type/buffer.c`.
//!
//! Ported subset of the upstream `Buffer` object (`buffer.h:10-160`). The invariants that upstream
//! relies on and that later crates (pack codec, crypto, io) will depend on are kept exactly:
//!
//! - `used() <= size()` always (upstream `bufUsed() <= bufSize()`, asserted in `bufUsedInc()`).
//! - Appending past the current size grows the allocation to exactly what is required
//!   (upstream `bufCatC()` calls `bufResize(this, used + catSize)` — there is no amortized doubling).
//! - The buffer is **not** NUL-terminated and `used()` never counts a terminator
//!   (nothing in `buffer.c` writes or reserves a NUL).
//! - `size()` may be smaller than the allocation: upstream models this with `bufLimitSet()` and
//!   `bufSizeLimit()`. Here the clamped view is expressed by [`Buffer::shrink`] so fixed-size
//!   buffers like the 32 KiB `logBuffer` behave like the C.
//!
//! Not ported in this round (they belong to later phases): `bufNewDecode()` (base64/hex construction
//! lives in [`crate::encode`]), `bufFind()`, mem-context reparenting (`bufMove()`), and the
//! compile-time `BUFFER_*` static helpers, all of which have no caller yet.

/// Byte buffer with `used`/`size` semantics matching upstream `Buffer`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Buffer {
    /// Payload bytes. Upstream `buffer` + `sizeAlloc`.
    data: Vec<u8>,
    /// Bytes in use. Upstream `used`.
    used: usize,
    /// Reported size, i.e. the write limit. Upstream `size`.
    size: usize,
}

impl Buffer {
    /// An empty buffer with `capacity` bytes of reported size, `used` at zero.
    ///
    /// Mirrors `bufNew(size)` (`buffer.c:45-56`), except that upstream leaves a zero-size buffer
    /// with a NULL pointer; here the allocation simply stays empty.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            data: Vec::with_capacity(capacity),
            used: 0,
            size: capacity,
        }
    }

    /// A buffer that owns a copy of `source`, with `used()` equal to `source.len()`.
    ///
    /// Mirrors `bufNewC(source, size)` (`buffer.c:61-76`).
    #[must_use]
    pub fn from_bytes(source: &[u8]) -> Self {
        let mut result = Self::new(source.len());
        result.data.extend_from_slice(source);
        result.used = source.len();
        result
    }

    /// A buffer over the bytes of a string, without the terminator (`bufNewC(str, strSize(str))`).
    // Not implementing `std::str::FromStr`; the name mirrors upstream's `bufNewC` surface and
    // takes a `&str` only so callers can skip the `.as_bytes()`.
    #[allow(clippy::should_implement_trait)]
    #[must_use]
    pub fn from_str(source: &str) -> Self {
        Self::from_bytes(source.as_bytes())
    }

    /// Bytes currently in use. Mirrors `bufUsed()`.
    #[must_use]
    pub fn used(&self) -> usize {
        self.used
    }

    /// Reported size, i.e. the write limit. Mirrors `bufSize()`.
    #[must_use]
    pub fn size(&self) -> usize {
        self.size
    }

    /// Bytes still available before the buffer is full. Mirrors `bufRemains()` (`size - used`).
    #[must_use]
    pub fn remains(&self) -> usize {
        self.size - self.used
    }

    /// True when nothing is in use. Mirrors `bufEmpty()`.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.used == 0
    }

    /// True when `used() == size()`. Mirrors `bufFull()`.
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.used == self.size
    }

    /// The payload. Mirrors `bufPtrConst()` + `bufUsed()`.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.data[..self.used]
    }

    /// Append `source`, growing the buffer as needed.
    ///
    /// Mirrors `bufCatC()` (`buffer.c:149-159`): when the data does not fit, the buffer is resized
    /// to exactly `used + source.len()`, so `size()` grows to the demand rather than by a growth
    /// factor. Returns the number of bytes appended.
    #[must_use = "appending bytes has no effect unless the result is used"]
    pub fn append(&mut self, source: &[u8]) -> usize {
        if self.used + source.len() > self.size {
            self.size = self.used + source.len();
        }

        if self.data.len() < self.size {
            self.data.resize(self.size, 0);
        }

        let start = self.used;
        self.data[start..start + source.len()].copy_from_slice(source);
        self.used += source.len();
        source.len()
    }

    /// Append a single byte. Convenience over [`Buffer::append`], used by the fixed-size write
    /// helpers below (`logBuffer[pos++] = char`).
    pub fn push(&mut self, byte: u8) {
        // The returned count is always 1 here, so it is deliberately discarded.
        let _ = self.append(&[byte]);
    }

    /// Set the number of bytes in use without touching the content.
    ///
    /// Mirrors `bufUsedSet(used)` (`buffer.c:355-366`), which asserts `used <= bufSize()`. A Rust
    /// panic stands in for the C `ASSERT()` because an inconsistent buffer is a bug in the caller.
    ///
    /// # Panics
    /// If `used` is larger than [`Buffer::size`], matching the upstream assertion.
    pub fn set_used(&mut self, used: usize) {
        assert!(used <= self.size, "assertion 'used <= bufSize()' failed");
        self.used = used;
    }

    /// Clamp the reported size to `size`, keeping bytes already in use.
    ///
    /// Mirrors `bufLimitSet()`/the `bufResize()` clamp (`buffer.c:257-292`): `used()` is preserved
    /// and `size()` becomes the write limit used by [`Buffer::write_truncated`].
    ///
    /// # Panics
    /// If `size` is smaller than [`Buffer::used`], matching the upstream `bufLimitSet()` assertion.
    pub fn shrink(&mut self, size: usize) {
        assert!(size >= self.used, "assertion 'limit >= bufUsed()' failed");
        self.size = size;
    }

    /// Append at most `self.size() - self.used()` bytes of `source` and report whether the tail was
    /// dropped.
    ///
    /// This is the `snprintf()` behavior the C log and error message buffers depend on: the write is
    /// clamped to the buffer (`logPost()` clamps `bufferPos` to `LOG_BUFFER_SIZE - 2`,
    /// `errorInternalThrowSysFmt()` clamps `messageSize` to `ERROR_MESSAGE_BUFFER_SIZE - 1`) instead
    /// of growing. Unlike [`Buffer::append`] this never changes `size()`.
    ///
    /// Returns `true` when some bytes of `source` did not fit.
    #[must_use = "truncation must be handled by the caller"]
    pub fn append_clamped(&mut self, source: &[u8]) -> bool {
        let available = self.remains();
        let written = available.min(source.len());
        let start = self.used;

        if self.data.len() < self.size {
            self.data.resize(self.size, 0);
        }

        self.data[start..start + written].copy_from_slice(&source[..written]);
        self.used += written;
        written < source.len()
    }

    /// Interpret the payload as UTF-8 lossily. Used by log/message rendering, never on the wire.
    #[must_use]
    pub fn to_string_lossy(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(self.bytes())
    }
}
