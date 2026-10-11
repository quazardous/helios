//! Configuration layers kept in the vault: the `user` layer (it follows the
//! user to every machine that opens the vault) and each project's layer
//! (`project/<id>`), as the TOML text `helios-config` reads. The vault only
//! keeps the text; the program's edge hands it to the configuration, which
//! still reads nothing behind the caller's back.

use serde_json::json;

use crate::{Store, Vault};

const NS: &str = "config";

impl<S: Store> Vault<S> {
    /// A configuration layer's TOML text, if the vault keeps one.
    pub fn config_layer(&self, name: &str) -> Result<Option<String>, String> {
        let doc = self.store.lock().expect("store lock").document(NS, name)?;
        doc.map(|d| {
            d.get("toml")
                .and_then(|t| t.as_str())
                .map(String::from)
                .ok_or_else(|| format!("config layer `{name}` holds no text"))
        })
        .transpose()
    }

    /// Keeps (or with `None` forgets) a configuration layer's TOML text.
    pub fn set_config_layer(&self, name: &str, toml: Option<&str>) -> Result<(), String> {
        let doc = toml.map(|t| json!({ "toml": t }));
        self.store
            .lock()
            .expect("store lock")
            .set_document(NS, name, doc.as_ref())
    }
}
