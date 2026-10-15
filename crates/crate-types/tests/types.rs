//! Behavior tests for the type primitives, pinned against upstream buffer.c and encode.c.

use crate_types::{Buffer, decode_hex, encode_hex};

#[test]
fn buffer_new_reports_capacity_as_size() {
    let buffer = Buffer::new(4);
    assert_eq!(buffer.size(), 4);
    assert_eq!(buffer.used(), 0);
    assert!(buffer.is_empty());
    assert!(!buffer.is_full());
    assert_eq!(buffer.remains(), 4);
    assert_eq!(buffer.bytes(), b"");
}

#[test]
fn buffer_append_grows_exactly_like_buf_cat_c() {
    // bufCatC() resizes to exactly used + catSize: no amortized doubling.
    let mut buffer = Buffer::new(4);
    assert_eq!(buffer.append(b"hello"), 5, "append returns bytes appended");
    assert_eq!(buffer.used(), 5);
    assert_eq!(buffer.size(), 5, "size grows to exactly the demand");
    assert_eq!(buffer.bytes(), b"hello");
}

#[test]
fn buffer_from_str_excludes_terminator() {
    let buffer = Buffer::from_str("abc");
    assert_eq!(buffer.used(), 3);
    assert_eq!(buffer.bytes(), b"abc");
}

#[test]
fn buffer_is_not_nul_terminated() {
    let buffer = Buffer::from_bytes(b"a\0b");
    assert_eq!(buffer.used(), 3);
    assert_eq!(buffer.bytes(), b"a\0b");
}

#[test]
fn buffer_append_clamped_truncates_without_growing() {
    let mut buffer = Buffer::new(2);
    let truncated = buffer.append_clamped(b"hello");
    assert!(truncated, "overflow must be reported");
    assert_eq!(buffer.used(), 2);
    assert_eq!(buffer.size(), 2, "clamped append never changes size");
    assert_eq!(buffer.bytes(), b"he");
    assert!(buffer.is_full());
}

#[test]
fn buffer_append_clamped_fits() {
    let mut buffer = Buffer::new(4);
    assert!(!buffer.append_clamped(b"ab"));
    assert_eq!(buffer.bytes(), b"ab");
}

#[test]
fn buffer_shrink_limits_writes() {
    let mut buffer = Buffer::new(10);
    assert!(!buffer.append_clamped(b"0123"));
    buffer.shrink(6); // limit >= used (4) holds, matching bufLimitSet()'s assert
    assert_eq!(buffer.size(), 6);
    assert!(buffer.append_clamped(b"xxxxxxxxxx"));
    assert_eq!(buffer.used(), 6, "writes beyond the limit are dropped");
    assert_eq!(buffer.bytes(), b"0123xx");
}

#[test]
#[should_panic(expected = "used <= bufSize")]
fn buffer_set_used_asserts_like_c() {
    let mut buffer = Buffer::new(2);
    buffer.set_used(3);
}

#[test]
fn hex_encode_is_lowercase_and_unseparated() {
    assert_eq!(encode_hex(b""), "");
    assert_eq!(encode_hex(b"abc"), "616263");
    assert_eq!(encode_hex(&[0x00, 0xff, 0x10]), "00ff10");
}

#[test]
fn hex_decode_round_trips() {
    assert_eq!(decode_hex(b"").expect("empty"), b"");
    assert_eq!(decode_hex(b"616263").expect("valid"), b"abc");
    assert_eq!(
        decode_hex(b"00FF10").expect("uppercase ok"),
        [0x00, 0xff, 0x10]
    );
}

#[test]
fn hex_decode_reports_odd_length_first() {
    // Upstream checks length before characters (decodeToBinValidateHex).
    let error = decode_hex(b"abc").expect_err("odd length must fail");
    assert_eq!(error.code(), 29, "upstream FormatError");
    assert_eq!(error.message(), "hex size 3 is not evenly divisible by 2");
}

#[test]
fn hex_decode_reports_first_invalid_position() {
    let error = decode_hex(b"0g").expect_err("invalid char must fail");
    assert_eq!(error.message(), "hex invalid character found at position 1");
}
