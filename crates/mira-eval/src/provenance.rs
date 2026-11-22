//! Content identity and archived sample evidence. The snapshot travels in the
//! transcript's open metadata so execution artifacts stay replayable without
//! tying the host to a study's dataset implementation.

use sha2::{Digest, Sha256};

/// Reserved transcript metadata key for the exact sample used during execution.
pub const SAMPLE: &str = "mira.sample";

/// Stable SHA-256 content fingerprint, prefixed with the algorithm name.
pub fn fingerprint(bytes: &[u8]) -> String {
    let hex: String = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("sha256:{hex}")
}

#[cfg(test)]
mod tests {
    #[test]
    fn standard_digest() {
        assert_eq!(
            super::fingerprint(b"abc"),
            "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
