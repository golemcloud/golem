//! The bytes a shell string stands for, and the shell string that stands for some bytes.
//!
//! Bash's strings are byte strings, but Brush keeps its values as Rust strings: each byte that is
//! not part of valid UTF-8 is a character of its own (see `brush_core::rawbytes`). Every value
//! that leaves the shell for a command, or comes back into the shell from one, is converted here,
//! so that the commands have one conversion point.

use std::{borrow::Cow, ffi::OsString};

/// The bytes the shell string `text` stands for.
pub(crate) fn encode(text: &str) -> Cow<'_, [u8]> {
    brush_core::rawbytes::encode(text)
}

/// The shell string that stands for `bytes`.
#[cfg_attr(
    not(target_arch = "wasm32"),
    allow(
        dead_code,
        reason = "only wasm32's printf %q decodes an argument's bytes"
    )
)]
pub(crate) fn decode(bytes: &[u8]) -> Cow<'_, str> {
    brush_core::rawbytes::decode(bytes)
}

/// The shell string that stands for `bytes`, reusing the buffer when it can.
pub(crate) fn decode_vec(bytes: Vec<u8>) -> String {
    brush_core::rawbytes::decode_vec(bytes)
}

/// The shell string `text` as an operating-system string: the bytes it stands for.
pub(crate) fn to_os_string(text: &str) -> OsString {
    brush_core::rawbytes::to_os_string(text)
}

/// The shell string `text` as UTF-8 text, each byte of it that is not UTF-8 replaced by U+FFFD:
/// what a command that reads only text (jq) makes of the bytes.
pub(crate) fn to_utf8_lossy(text: &str) -> String {
    String::from_utf8_lossy(&encode(text)).into_owned()
}

/// The byte that the single character `c` of a shell string stands for, when it stands for a
/// byte that is not part of valid UTF-8.
pub(crate) fn raw_byte(c: char) -> Option<u8> {
    brush_core::rawbytes::char_byte(c)
}

#[cfg(test)]
mod tests {
    use super::{decode, decode_vec, encode, raw_byte, to_os_string, to_utf8_lossy};

    #[test]
    fn text_round_trips() {
        for text in ["", "plain", "é ✓", "tab\there\n"] {
            assert_eq!(decode_vec(encode(text).into_owned()), text);
            assert_eq!(decode(&encode(text)), text);
            assert_eq!(to_utf8_lossy(text), text);
            assert!(text.chars().all(|c| raw_byte(c).is_none()));
        }
    }

    #[test]
    fn bytes_that_are_not_utf8_round_trip() {
        let text = decode_vec(b"a\xffb\xc3".to_vec());
        assert_eq!(text.chars().count(), 4);
        assert_eq!(encode(&text), &b"a\xffb\xc3"[..]);
        assert_eq!(decode(b"a\xffb\xc3"), text);
        assert_eq!(to_os_string(&text).as_encoded_bytes(), b"a\xffb\xc3");
        let bytes: Vec<u8> = text.chars().filter_map(raw_byte).collect();
        assert_eq!(bytes, [0xFF, 0xC3]);
        // What jq makes of them: U+FFFD for each.
        assert_eq!(to_utf8_lossy(&text), "a\u{FFFD}b\u{FFFD}");
    }
}
