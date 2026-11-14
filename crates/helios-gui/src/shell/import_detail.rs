//! The import window's Detail column, the right zone
//! (`docs/design/import-window.md`): the selected source's connector in
//! detail, as Lightroom's and darktable's import dialogs keep their
//! options on the right of the grid.
//!
//! - **At the top, the choices every source shares**: duplicates, the
//!   darktable edits (when the connector honours them), More.
//! - **Below, the connector's own section**, under its name: what the
//!   connector declares calls for each part ([`ImportWindow::detail`]),
//!   one contract for every connector (folders, cards, Google Photos, the
//!   plugins'):
//!   - an account connector: its account, and for a connector whose
//!     photos are chosen in its service's page, the connection and
//!     "Choose photos" ([`super::import_accounts::choose_ui`]);
//!   - copy or add where they are (a connector that cannot add in place
//!     always copies), then Into and Name for a copy;
//!   - the connector's own parameters: the keys of its source contract
//!     that are not choices, drawn from the contract with the same
//!     widgets as Settings and the "Add a source…" wizard, kept on the
//!     source as they change.
//! - **The backup line**, last.
//!
//! The left zone lists the sources, the middle one shows their photos.

use serde_json::{Map, Value};

use helios_core::Message;
use helios_core::config::Level;

use super::import_sources::SourceKey;
use super::import_window::ImportWindow;
use super::import_wizard::{contract_ui, own_params, refused_ui};

/// One part of a connector's section, as its declarations call for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Part {
    /// An account's source: its connection, and "Choose photos" when the
    /// connector's photos are chosen in its service's page (`picker`).
    Account { id: String, picker: bool },
    /// Copy or add where they are; Into and Name for a copy.
    Copy,
    /// The connector's own parameters (its source contract's keys that
    /// are not choices).
    Params,
}

/// The selected source's connector section: its name and its parts, in
/// the order drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Detail {
    /// The source it is for: the account clicked last, else the folder
    /// ticked last; none before any is.
    pub key: Option<SourceKey>,
    /// Its connector's id.
    pub connector: String,
    /// Its connector's name, translated: the section's heading.
    pub label: String,
    pub parts: Vec<Part>,
}

impl ImportWindow {
    /// The Detail column's connector section for the selected source,
    /// from what its connector declares: its browsers (a service's page
    /// to choose in), its source contract (its own parameters). Every
    /// connector goes through it; none draws the column itself.
    pub(super) fn detail(&mut self) -> Detail {
        let key = self
            .sources
            .account
            .clone()
            .map(SourceKey::Account)
            .or_else(|| self.ticked.last().cloned().map(SourceKey::Folder));
        let connector = key.as_ref().map_or_else(
            || helios_connect::folder::ID.to_string(),
            |k| self.connector_of(k),
        );
        let provider = self.connectors.provider(&connector);
        let label = provider.as_ref().map_or_else(
            || connector.clone(),
            |p| super::import_accounts::translated(self.texts.as_ref(), &p.manifest().label),
        );
        let mut parts = Vec::new();
        if let Some(SourceKey::Account(id)) = &key {
            let picker = provider.as_ref().is_some_and(|p| {
                p.manifest()
                    .browsers
                    .contains(&helios_connect::Browser::Picker)
            });
            parts.push(Part::Account {
                id: id.clone(),
                picker,
            });
        }
        parts.push(Part::Copy);
        if key.is_some()
            && provider
                .as_ref()
                .is_some_and(|p| !own_params(&p.config().source).is_empty())
        {
            parts.push(Part::Params);
        }
        Detail {
            key,
            connector,
            label,
            parts,
        }
    }

    /// The connector's own parameters of the source of `detail`: its
    /// saved values over the contract's defaults, kept on the source as
    /// they change, once they fit the contract.
    pub(super) fn detail_params_ui(&mut self, ui: &mut egui::Ui, detail: &Detail) {
        let (Some(key), Some(texts)) = (detail.key.clone(), self.texts.clone()) else {
            return;
        };
        let Some(provider) = self.connectors.provider(&detail.connector) else {
            return;
        };
        let contract = own_params(&provider.config().source);
        let mut values: Map<String, Value> = contract.defaults();
        if let Some(saved) = self.sources.get(&key) {
            for (k, v) in &saved.params {
                if contract.get(k).is_some() {
                    values.insert(k.clone(), v.clone());
                }
            }
        }
        let before = values.clone();
        // Whose catalogue translates them: the plugin behind the
        // connector, else the connector.
        let owner = self
            .accounts
            .plugins
            .as_ref()
            .and_then(|b| helios_plugin::source::plugin_of(b, &detail.connector))
            .unwrap_or_else(|| detail.connector.clone());
        let t = |m: Message| texts(m);
        ui.strong(t(Message::new("gui-editor-params", "Parameters")));
        let fit = contract_ui(ui, &t, &owner, Level::Source, &contract, &mut values);
        let fits = fit.is_ok();
        refused_ui(ui, &t, fit);
        if fits && values != before {
            self.sources
                .set_params(&key, &detail.connector, values.into_iter().collect());
        }
    }
}
