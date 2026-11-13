//! The claimed-hash table (`docs/design/import-connectors.md`, section
//! 2.7): a hash a connector states for an item, in its own algorithm
//! (MD5, SHA-1, SHA-256, a service's own), mapped to the image whose
//! bytes were checked against it. Before a download, a claim found here
//! names the image without fetching a byte.
//!
//! A claim is written only once Helios computed the same digest over the
//! bytes it received (or over a Library file), never on a connector's
//! word alone. Kept as documents in the namespace [`CLAIMS`], one per
//! `<algorithm>:<digest>`, so the table travels with the library and
//! needs no schema step.

use serde_json::json;

use crate::{ImageId, Store, Vault};

/// The documents' namespace.
pub const CLAIMS: &str = "import/claims";

/// A claim's key: its algorithm and its digest, in lower case.
fn key(algorithm: &str, digest: &str) -> String {
    format!(
        "{}:{}",
        algorithm.to_ascii_lowercase(),
        digest.to_ascii_lowercase()
    )
}

impl<S: Store> Vault<S> {
    /// The image a claim was checked against, if any.
    pub fn claimed(&self, algorithm: &str, digest: &str) -> Result<Option<ImageId>, String> {
        let doc = self.read(|s| s.document(CLAIMS, &key(algorithm, digest)))?;
        doc.and_then(|d| d["image"].as_str().map(ImageId::from_hex))
            .transpose()
    }

    /// Records that the bytes of `image` hash to `digest` in `algorithm`:
    /// what Helios computed, not what a source said.
    pub fn set_claim(&self, algorithm: &str, digest: &str, image: &ImageId) -> Result<(), String> {
        self.store.lock().expect("store lock").set_document(
            CLAIMS,
            &key(algorithm, digest),
            Some(&json!({ "image": image.hex() })),
        )
    }
}

#[cfg(test)]
mod tests {
    use helios_core::ContentHash;

    use crate::MemoryStore;

    use super::*;

    #[test]
    fn a_claim_names_its_image_whatever_the_digest_case() {
        let vault = Vault::new(MemoryStore::default());
        let image = ImageId(ContentHash([7; 32]));
        assert_eq!(vault.claimed("md5", "ABCD").unwrap(), None);
        vault.set_claim("MD5", "ABCD", &image).unwrap();
        assert_eq!(vault.claimed("md5", "abcd").unwrap(), Some(image));
        // Another algorithm with the same digest is another claim.
        assert_eq!(vault.claimed("sha1", "abcd").unwrap(), None);
    }
}
