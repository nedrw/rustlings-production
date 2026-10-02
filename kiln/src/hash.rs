//! SHA-256 of files, in the one text form the course writes down.
//!
//! Two mechanisms depend on the same hash and the same spelling: the `source_hash` of a translated
//! step body (plan §3 D5) and the `spec/manifest.toml` that pins the read-only spec files (§3 D2).
//! They share it here so a future third user cannot invent a second format.

use std::path::Path;

use sha2::{Digest, Sha256};

use crate::error::{KilnError, Result};

/// Format every hash is written in: `sha256:<64 hex characters>`.
pub const PREFIX: &str = "sha256:";

/// Hash bytes.
#[must_use]
pub fn bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut text = String::with_capacity(PREFIX.len() + 64);
    text.push_str(PREFIX);
    for byte in digest {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

/// Hash a file, naming it when it cannot be read.
pub fn file(path: &Path) -> Result<String> {
    let content = std::fs::read(path).map_err(|source| KilnError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(bytes(&content))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_use_the_sha256_lowercase_hex_form() {
        // The empty-input digest, from the SHA-256 specification.
        assert_eq!(
            bytes(b""),
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            bytes(b"abc"),
            "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn hashing_a_file_uses_the_same_form() {
        let temp = crate::test_support::TempDir::new("hash-file");
        let path = temp.write("a.txt", "abc");
        assert_eq!(file(&path).unwrap(), bytes(b"abc"));
        let missing = file(&temp.path().join("nope.txt")).unwrap_err();
        assert!(missing.to_string().contains("nope.txt"), "{missing}");
    }
}
