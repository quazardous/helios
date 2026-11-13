//! Calibration profiles (`docs/design/calibration.md`, section 6): one
//! document each in the namespace [`PROFILES`], keyed by the profile's id.
//! The vault keeps them as JSON; their shape is the calibration's
//! (`helios_calibrate::profile`).
//!
//! A save or a removal answers its [`Changes`] (the document before and
//! after), so it is undone and redone as the Library's albums are
//! ([`Vault::revert`], [`Vault::replay`]).

use serde_json::Value;

use crate::undo::{Changes, DocumentChange};
use crate::{Store, Vault};

/// The profiles' namespace.
pub const PROFILES: &str = "calibration/profiles";

impl<S: Store> Vault<S> {
    /// Every calibration profile, by id.
    pub fn calibration_profiles(&self) -> Result<Vec<(String, Value)>, String> {
        self.store.lock().expect("store lock").documents(PROFILES)
    }

    /// One calibration profile.
    pub fn calibration_profile(&self, id: &str) -> Result<Option<Value>, String> {
        self.store
            .lock()
            .expect("store lock")
            .document(PROFILES, id)
    }

    /// Keeps `profile` under `id` (or with `None` removes it), answering
    /// the change: none when it held the same already.
    pub fn set_calibration_profile(
        &self,
        id: &str,
        profile: Option<&Value>,
    ) -> Result<Changes, String> {
        self.transaction(|store| {
            let before = store.document(PROFILES, id)?;
            let after = profile.cloned();
            if before == after {
                return Ok(Changes::default());
            }
            store.set_document(PROFILES, id, after.as_ref())?;
            Ok(Changes {
                documents: vec![DocumentChange {
                    ns: PROFILES.into(),
                    key: id.into(),
                    before,
                    after,
                }],
                ..Changes::default()
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::{MemoryStore, Vault};

    #[test]
    fn a_profile_is_kept_removed_and_brought_back_by_its_changes() {
        let vault = Vault::new(MemoryStore::default());
        let profile = json!({ "format": 1, "name": "Flash" });
        let saved = vault.set_calibration_profile("ab", Some(&profile)).unwrap();
        assert_eq!(
            vault.calibration_profile("ab").unwrap(),
            Some(profile.clone())
        );
        assert!(
            vault
                .set_calibration_profile("ab", Some(&profile))
                .unwrap()
                .is_empty()
        );
        let removed = vault.set_calibration_profile("ab", None).unwrap();
        assert!(vault.calibration_profiles().unwrap().is_empty());
        vault.revert(&removed).unwrap();
        assert_eq!(
            vault.calibration_profiles().unwrap(),
            vec![("ab".into(), profile)]
        );
        vault.revert(&saved).unwrap();
        assert_eq!(vault.calibration_profile("ab").unwrap(), None);
        vault.replay(&saved).unwrap();
        assert!(vault.calibration_profile("ab").unwrap().is_some());
    }
}
