//! A short tag for a value, so two versions of a variable can be told apart (or
//! seen to be equal) on screen without printing the value.
//!
//! It is for display only and is never stored: history keeps the values
//! themselves, and recomputes the tag whenever it prints one.

use sha2::{Digest, Sha256};

/// Six hex characters. Equal values always give equal tags; different values
/// differ except for about one pair in sixteen million.
pub fn fingerprint(value: &str) -> String {
    fingerprint_bytes(value.as_bytes())
}

/// The same tag for arbitrary bytes, such as the contents of a key file.
pub fn fingerprint_bytes(value: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(b"dotvault-fingerprint\0");
    hash.update(value);
    let digest = hash.finalize();
    digest.iter().take(3).map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_values_share_a_tag_and_different_values_do_not() {
        assert_eq!(fingerprint("postgres://x"), fingerprint("postgres://x"));
        assert_ne!(fingerprint("postgres://x"), fingerprint("postgres://y"));
        assert_ne!(fingerprint(""), fingerprint(" "));
    }

    #[test]
    fn bytes_and_text_agree_and_binary_data_works() {
        assert_eq!(fingerprint("abc"), fingerprint_bytes(b"abc"));
        assert_ne!(
            fingerprint_bytes(&[0, 255, 0]),
            fingerprint_bytes(&[0, 255, 1])
        );
    }

    #[test]
    fn the_tag_is_six_hex_characters_and_does_not_contain_the_value() {
        let tag = fingerprint("hunter2");
        assert_eq!(tag.len(), 6);
        assert!(tag.chars().all(|c| c.is_ascii_hexdigit()), "{tag}");
        assert!(!tag.contains("hunter"));
    }
}
