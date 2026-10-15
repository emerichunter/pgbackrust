//! PgBackRust type primitives ported from pgBackRest `src/common/type`.
//!
//! Scope for this round is deliberately minimal: the byte buffer and the byte/hex helpers that
//! `crate-error` and `crate-log` need. The upstream `String`, `List`, `Kv` and `VariantList` types
//! are not ported here.
//!
//! Parity notes (AGENTS.md R5):
//!
//! - [`Buffer`] mirrors `src/common/type/buffer.c`: `used <= size <= size_alloc`, a `size` that can
//!   be a clamped view of a larger allocation (upstream `bufSizeLimit()`/`bufResize()`), and a
//!   buffer that is **never** NUL-terminated (`bufUsed()` counts payload bytes only).
//! - Hex encoding matches `encodeToStrHex()` in `src/common/encode.c`: two lowercase characters per
//!   byte, no separators, no prefix.
//! - Hex decoding matches `decodeToBinValidateHex()`/`encodeToBin*` in `src/common/encode.c`: both
//!   `0-9a-f` and `0-9A-F` are accepted, everything else is rejected, and the error message text is
//!   byte-identical to the upstream `FormatError` messages.
//!
//! Documented deviation from the C: upstream throws `FormatError` from the hex decoder. Throwing
//! requires `crate-error`, and `crate-error` sits above the type primitives in the upstream
//! dependency order, so a dependency would be circular. [`EncodeError`] therefore carries the same
//! message text and reports the same error code ([`EncodeError::code`] = 29, `format`) so callers
//! can rethrow it through `crate-error` unchanged.

pub mod buffer;
pub mod encode;

pub use buffer::Buffer;
pub use encode::{EncodeError, decode_hex, encode_hex};
