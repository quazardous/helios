//! Storage mounted in the desktop: the feature lives in its own crate
//! (`helios_storage_ui`: the Storage and Backup pages, their commands, a
//! backup's end, the cards safe to format) and reaches the rest of the
//! desktop through the bus. What stays here is the mounting: its commands
//! put in the shell's registry, its listeners registered on the
//! application's bus, how a card safe to format is said ([`Told`]), the
//! saved sources it names remote volumes by ([`Sources`]), the import's
//! end it waits on, and its step each frame.
//!
//! `library.storage` opens the Settings app at Storage › Volumes;
//! `{"volume": id}` marks that volume and scrolls to it (the health
//! check's free-space fix does).

use std::rc::Rc;
use std::sync::{Arc, PoisonError};

use egui_pop::api::toasts::Toast;
use helios_core::Message;
use helios_storage::Volume;
use helios_storage::strategy::Strategy;
use helios_storage_ui::admin::{self, VOLUMES};
use helios_storage_ui::backup_end::CardList;
use helios_storage_ui::page::{SourceBook, SourceUse, Tell};
use helios_storage_ui::{StorageLink, page};
use serde_json::Value;

use super::Shell;
use super::import_sources::SharedSources;
use crate::import_end;

/// The import window's saved sources, as the Storage page reads them.
pub(super) struct Sources(pub(super) SharedSources);

impl SourceBook for Sources {
    fn sources(&self) -> Vec<SourceUse> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|s| SourceUse {
                account: s.account.clone(),
                name: s.name.clone(),
            })
            .collect()
    }
}

/// How the desktop says a card safe to format: a success toast, the Log,
/// the notes.
struct Told {
    teller: super::said::Teller,
    log: Rc<std::cell::RefCell<crate::log_panel::Log>>,
}

impl Tell for Told {
    fn success(&self, message: Message) {
        let text = self.teller.text(message.clone());
        self.teller.toast(Toast::success(&text));
        self.log
            .borrow_mut()
            .push(crate::dialogs::Level::Info, message);
        self.teller.note(text);
    }
}

impl Shell {
    /// Reads and writes volumes through `link`: the Storage page, and the
    /// names of offline photos' volumes in the Library.
    pub fn set_storage(&mut self, link: Arc<dyn StorageLink>) {
        self.register_storage_commands(&link);
        self.core
            .storage
            .attach_link(link, &*self.core.app.handle().bus());
    }

    /// Registers the Storage page's commands over `link`, once.
    fn register_storage_commands(&mut self, link: &Arc<dyn StorageLink>) {
        if self.has_command(VOLUMES) {
            return;
        }
        let i18n = self.core.i18n.clone();
        let (owners, cards) = (
            self.core.storage.owners.clone(),
            self.core.storage.cards.clone(),
        );
        admin::register_commands(
            &mut |spec, handler| self.register(spec, handler),
            link,
            &i18n,
            (&owners, &cards),
        );
    }

    /// Reads the volumes again: the page's list, and the volumes the
    /// Library names (a scan ran, a disk came or went). What the link
    /// reads off the frame shows at a coming one.
    pub fn refresh_volumes(&mut self) {
        self.core
            .storage
            .refresh_volumes(&*self.core.app.handle().bus());
    }

    /// Each photo's backups read again (one read for the Library), for
    /// its badges and its `backup:` filters; what the link reads off the
    /// frame shows at a coming one.
    pub fn refresh_backups(&mut self) {
        self.core
            .storage
            .refresh_backups(&*self.core.app.handle().bus());
    }

    /// `storage.cards-waiting`, the shell's listener of `import.ended`:
    /// the cards an import read, from its report (`cards`), those short
    /// of their backups waiting for a backup job to end.
    pub(super) fn listen_cards_waiting(&self) -> Result<(), String> {
        let cards = self.core.storage.cards.clone();
        import_end::listen_local(
            &*self.core.app.handle().bus(),
            import_end::CARDS,
            move |end, _| {
                if let Ok(report) = &end.outcome {
                    cards.wait_for_backups(&report.cards);
                }
            },
        )
    }

    /// The one card list: the cards an import left short of their
    /// backups, while inserted (the health check's thread reads it).
    pub fn waiting_cards(&self) -> CardList {
        self.core.storage.cards.clone()
    }

    /// The cards still waiting for their backups.
    pub fn cards_waiting_for_backups(&self) -> Vec<String> {
        self.core
            .storage
            .cards
            .waiting()
            .into_iter()
            .map(|c| c.label)
            .collect()
    }

    /// Storage's listeners of `backup.ended` and `card.safe`
    /// (`helios_storage_ui::backup_end`), each in its declared place; the
    /// import window reads the same card list.
    pub(super) fn listen_backup_points(&mut self) {
        self.import.get_mut().card_list = self.core.storage.cards.clone();
        let told = Rc::new(Told {
            teller: self.teller(),
            log: self.core.log.clone(),
        });
        let refused = self
            .core
            .storage
            .listen_backup_points(self.core.app.handle(), told);
        self.notes.extend(refused);
    }

    /// A backup job ended: passed through `backup.ended`, then said on
    /// the journal ([`helios_storage_ui::backup_ended`]).
    pub(super) fn backup_ended(&mut self) {
        helios_storage_ui::backup_ended(self.core.app.handle());
    }

    /// Each frame: the node facts the Settings app's reads follow, what
    /// the link read since, and the cards checked since a backup ended.
    pub(super) fn storage_frame(&mut self) {
        self.reads_frame();
        if self.core.storage.frame(self.core.app.handle()) {
            self.absorb_said();
        }
    }

    /// The import window's strategy line, and whether it is the one
    /// without a strategy ("No backup set up:", the Storage page offered):
    /// "2 copies: Backup disk (plugged in), NAS (not seen for 3 days)".
    pub fn import_backup_line(&self) -> (String, bool) {
        self.core.storage.backup_line(&|m| self.text(m))
    }

    /// Whether the Settings app is open on its Storage or Backup scope.
    pub fn storage_page_open(&self) -> bool {
        page::page_open(self.core.app.handle())
    }

    /// The library's backup strategy, as last read.
    pub fn storage_strategy(&self) -> Option<Strategy> {
        self.core.storage.strategy()
    }

    /// The volumes known, online ones first.
    pub fn storage_volumes(&self) -> Vec<Volume> {
        self.core.storage.volumes()
    }

    /// `library.storage`: opens the Settings app at Storage › Volumes,
    /// on `volume` when given.
    pub(super) fn open_storage(&mut self, args: &Value) -> Result<Value, String> {
        self.refresh_volumes();
        let path = match args["volume"].as_str() {
            Some(v) => format!("storage/volumes/{v}"),
            None => "storage/volumes".into(),
        };
        self.open_settings_page(&path)
    }
}
