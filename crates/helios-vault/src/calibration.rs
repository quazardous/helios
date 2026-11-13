//! Calibration profiles (`docs/design/calibration.md`, section 6): one
//! document each in the namespace [`PROFILES`], keyed by the profile's id.
//! A camera's default (which profiles its photos may get) is one document
//! in [`DEFAULTS`], keyed by the camera's key. The vault keeps them as
//! JSON; their shape is the calibration's (`helios_calibrate::profile`,
//! `helios_calibrate::apply::Rule`).
//!
//! A save or a removal answers its [`Changes`] (the document before and
//! after), so it is undone and redone as the Library's albums are
//! ([`Vault::revert`], [`Vault::replay`]).

use serde_json::Value;

use crate::undo::{Changes, DocumentChange};
use crate::{Store, Vault};

/// The profiles' namespace.
pub const PROFILES: &str = "calibration/profiles";

/// The camera defaults' namespace.
pub const DEFAULTS: &str = "calibration/defaults";

impl<S: Store> Vault<S> {
    /// Every calibration profile, by id.
    pub fn calibration_profiles(&self) -> Result<Vec<(String, Value)>, String> {
        self.read(|s| s.documents(PROFILES))
    }

    /// One calibration profile.
    pub fn calibration_profile(&self, id: &str) -> Result<Option<Value>, String> {
        self.read(|s| s.document(PROFILES, id))
    }

    /// Keeps `profile` under `id` (or with `None` removes it), answering
    /// the change: none when it held the same already.
    pub fn set_calibration_profile(
        &self,
        id: &str,
        profile: Option<&Value>,
    ) -> Result<Changes, String> {
        self.set_calibration_document(PROFILES, id, profile)
    }

    /// A camera's default, by the camera's key.
    pub fn calibration_default(&self, camera: &str) -> Result<Option<Value>, String> {
        self.read(|s| s.document(DEFAULTS, camera))
    }

    /// Every camera default, by camera key.
    pub fn calibration_defaults(&self) -> Result<Vec<(String, Value)>, String> {
        self.read(|s| s.documents(DEFAULTS))
    }

    /// Keeps `rule` as the default of camera `camera` (or with `None`
    /// clears it), answering the change: none when it held the same.
    pub fn set_calibration_default(
        &self,
        camera: &str,
        rule: Option<&Value>,
    ) -> Result<Changes, String> {
        self.set_calibration_document(DEFAULTS, camera, rule)
    }

    fn set_calibration_document(
        &self,
        ns: &str,
        key: &str,
        doc: Option<&Value>,
    ) -> Result<Changes, String> {
        self.transaction(|store| {
            let before = store.document(ns, key)?;
            let after = doc.cloned();
            if before == after {
                return Ok(Changes::default());
            }
            store.set_document(ns, key, after.as_ref())?;
            Ok(Changes {
                documents: vec![DocumentChange {
                    ns: ns.into(),
                    key: key.into(),
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

    #[test]
    fn a_camera_default_is_kept_cleared_and_brought_back() {
        let vault = Vault::new(MemoryStore::default());
        let rule = json!({ "profiles": ["ab"], "pick": "nearest" });
        let set = vault
            .set_calibration_default("nikon z 6", Some(&rule))
            .unwrap();
        assert_eq!(set.documents[0].ns, super::DEFAULTS);
        assert_eq!(
            vault.calibration_default("nikon z 6").unwrap(),
            Some(rule.clone())
        );
        assert!(vault.calibration_profiles().unwrap().is_empty());
        let cleared = vault.set_calibration_default("nikon z 6", None).unwrap();
        assert!(vault.calibration_defaults().unwrap().is_empty());
        vault.revert(&cleared).unwrap();
        assert_eq!(
            vault.calibration_defaults().unwrap(),
            vec![("nikon z 6".into(), rule)]
        );
    }
}
