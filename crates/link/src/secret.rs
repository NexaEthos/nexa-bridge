//! Device proofs and local keys from OS entropy. Wiki: docs/wiki/managed-auth.md

use std::fmt::Write as _;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

#[doc = "Return `N` random bytes from the OS as lowercase hex.\n# Errors\nThe OS entropy source failing."]
pub fn random_hex<const N: usize>() -> Result<String> {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).context("read OS entropy")?;
    let mut hex = String::with_capacity(N * 2);
    for byte in bytes {
        write!(&mut hex, "{byte:02x}").context("encode random bytes")?;
    }
    Ok(hex)
}

#[doc = "A fresh 256-bit secret: a device proof or a local bridge key.\n# Errors\nThe OS entropy source failing."]
pub fn generate() -> Result<String> {
    random_hex::<32>()
}

/// The SHA-256 of a proof as hex — the only form that leaves the computer at enrollment.
#[must_use]
pub fn digest(proof: &str) -> String {
    let mut hex = String::with_capacity(64);
    for byte in Sha256::digest(proof.as_bytes()) {
        let _ = write!(&mut hex, "{byte:02x}");
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_is_256_bits_of_hex_and_its_digest_is_stable() -> Result<()> {
        let one = generate()?;
        assert_eq!(one.len(), 64);
        assert!(one.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(one, generate()?);
        assert_eq!(
            digest("proof"),
            "c1cda26362828b69266512052b97cb3729e3b052e4ade47c0a1e3383defe73c7"
        );
        Ok(())
    }
}
