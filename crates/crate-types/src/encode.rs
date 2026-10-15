//! Hex codec ported from pgBackRest `src/common/encode.c`.
//!
//! Only the hex branch of upstream's `EncodeType` is ported, because that is what `crate-error` and
//! `crate-log` need in this round (base64/base64url are used by the crypto and storage crates and
//! arrive with them).
//!
//! Byte-exact parity points, all from `src/common/encode.c`:
//!
//! - Encoding (`encodeToStrHex()`, `encode.c:300-325`) emits two **lowercase** digits per byte with
//!   no separators, prefix, or trailing newline; `encodeToStrSizeHex()` = `source_size * 2`.
//! - Decoding accepts `0-9a-f` **and** `0-9A-F` (`encodeToBinLookupHex`, `encode.c:340-365`).
//! - Validation runs before conversion (`decodeToBinValidateHex()`, `encode.c:367-390`) and the
//!   message text is reproduced verbatim: an odd length reports
//!   `hex size {n} is not evenly divisible by 2`, and the first bad byte reports
//!   `hex invalid character found at position {n}`. The upstream length check short-circuits before
//!   the character scan, so an input that is both odd and invalid reports the size error.
//! - Position is the byte index of the invalid character, formatted with `%u` upstream; a decimal
//!   index of a `usize` renders identically.

use std::fmt;

/// Error raised by the hex decoder.
///
/// Upstream raises `FormatError` (code 29) from `decodeToBinValidateHex()`. `crate-types` cannot
/// depend on `crate-error` (see the crate docs for why), so this type carries the upstream message
/// text and the upstream code so a caller can rethrow it as a `FormatError` without alteration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodeError(String);

impl EncodeError {
    /// Upstream error code for `format` (`build/error.yaml`), i.e. the code `FormatError` carries.
    pub const FORMAT_CODE: i32 = 29;

    /// The upstream error code this failure corresponds to.
    #[must_use]
    pub const fn code(&self) -> i32 {
        Self::FORMAT_CODE
    }

    /// Message text, byte-identical to the upstream `FormatError` message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for EncodeError {}

/// Encode bytes as lowercase hex, matching `encodeToStrHex()` (`encode.c:300-325`).
#[must_use]
pub fn encode_hex(source: &[u8]) -> String {
    // Upstream drives this from a 512-byte lookup table ("00".."ff"); the same output falls out of
    // the nibble table below and stays readable.
    const DIGITS: &[u8; 16] = b"0123456789abcdef";

    let mut result = String::with_capacity(source.len() * 2);

    for byte in source {
        result.push(char::from(DIGITS[usize::from(byte >> 4)]));
        result.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }

    result
}

/// Hex digit value, or `None` for anything outside `0-9a-fA-F` (`encodeToBinLookupHex`).
fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Decode lowercase or uppercase hex into bytes.
///
/// Validation follows `decodeToBinValidateHex()` (`encode.c:367-390`) and the conversion follows
/// the `encodeToBin*` loop: `destination[i] = high_nibble << 4 | low_nibble`.
///
/// # Errors
/// [`EncodeError`] when the input length is not even or contains a non-hex byte, with the upstream
/// message for that condition.
pub fn decode_hex(source: &[u8]) -> Result<Vec<u8>, EncodeError> {
    if source.len() % 2 != 0 {
        return Err(EncodeError(format!(
            "hex size {} is not evenly divisible by 2",
            source.len()
        )));
    }

    for (position, byte) in source.iter().enumerate() {
        if hex_value(*byte).is_none() {
            return Err(EncodeError(format!(
                "hex invalid character found at position {position}"
            )));
        }
    }

    Ok(source
        .chunks(2)
        .map(|pair| (hex_value(pair[0]).unwrap_or(0) << 4) | hex_value(pair[1]).unwrap_or(0))
        .collect())
}
