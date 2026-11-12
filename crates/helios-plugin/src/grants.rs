//! The capabilities the person allowed each plugin, kept in a settings
//! layer (the person's, `user`, so they follow them) as the table
//! [`TABLE`]: each plugin's id with the capabilities it was granted.
//!
//! ```toml
//! [plugin-grants]
//! "io.github.quazardous.helios.google-photos" = ["folder.write", "network"]
//! ```
//!
//! The broker is given them at start ([`crate::Broker::load_grants`]) and
//! hands them back each time they change ([`crate::Broker::on_grants`]).
//! A capability a plugin did not declare is not granted, so one its
//! updated manifest asks for anew is asked for again.

use std::collections::{BTreeMap, BTreeSet};

use toml::{Table, Value as Toml};

/// The layer's table that holds them.
pub const TABLE: &str = "plugin-grants";

/// Each plugin's granted capabilities, by plugin id.
pub type Grants = BTreeMap<String, BTreeSet<String>>;

/// The grants a layer's TOML text holds; what does not read is left out.
pub fn read(text: &str) -> Grants {
    let Ok(table) = text.parse::<Table>() else {
        return Grants::new();
    };
    let Some(Toml::Table(grants)) = table.get(TABLE) else {
        return Grants::new();
    };
    grants
        .iter()
        .filter_map(|(id, caps)| {
            let caps: BTreeSet<String> = caps
                .as_array()?
                .iter()
                .filter_map(|c| c.as_str().map(String::from))
                .collect();
            (!caps.is_empty()).then(|| (id.clone(), caps))
        })
        .collect()
}

/// A layer's TOML text with its grants replaced by `grants`, the rest
/// kept.
///
/// # Errors
/// The text is not TOML.
pub fn write(text: &str, grants: &Grants) -> Result<String, String> {
    let mut table: Table = text
        .parse()
        .map_err(|e: toml::de::Error| format!("the settings layer is not TOML: {}", e.message()))?;
    let held: Table = grants
        .iter()
        .filter(|(_, caps)| !caps.is_empty())
        .map(|(id, caps)| {
            let caps = caps.iter().map(|c| Toml::String(c.clone())).collect();
            (id.clone(), Toml::Array(caps))
        })
        .collect();
    if held.is_empty() {
        table.remove(TABLE);
    } else {
        table.insert(TABLE.into(), Toml::Table(held));
    }
    toml::to_string(&table).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grants_are_written_beside_the_settings_and_read_back() {
        let layer = "[interface.graph]\nzoom = \"overview\"\n";
        let mut grants = Grants::new();
        grants.insert(
            "org.example.p".into(),
            ["network".to_string(), "folder.write".to_string()].into(),
        );
        let text = write(layer, &grants).unwrap();
        assert!(text.contains("zoom = \"overview\""), "{text}");
        assert_eq!(read(&text), grants);
        // None left: the table goes.
        let none = write(&text, &Grants::new()).unwrap();
        assert!(!none.contains(TABLE), "{none}");
        assert!(read("not = [toml").is_empty());
    }
}
