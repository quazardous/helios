//! A backup's end, and the cards safe to format, from one card list.
//!
//! The cards an import left short of their backups are kept in one list,
//! [`CardList`]: the import's end puts them there
//! (`storage.cards-waiting`), a backup's end checks them again, and every
//! reader (the import window's card rows, the Storage page's, the health
//! check's "Cards safe to format") reads that list, so they cannot
//! disagree.
//!
//! A backup job's end passes once through the hook point [`BackupEnded`],
//! its listeners in the order [`ORDER`] declares: the badges read again
//! (`storage.backups`), then the cards still inserted checked again off
//! the frame (`cards.safety`: the check reads the vault). When the check
//! lands, the shell marks the cards in the list and passes the cards it
//! found safe through [`CardSafe`] ([`SAFE_ORDER`]): the toast and the Log
//! (`cards.toast`), the Storage page's reads (`storage.cards`), the health
//! check's cards (`health.cards`), all in the pass the list changed. Both
//! are said on the journal (`backup.ended`, `card.safe`) with the
//! listeners that ran (`ran`).
//!
//! The listeners hold the interface's state (`Rc`): they are local
//! listeners ([`listen_local`]), bound to the interface's thread, where
//! the shell passes both points.

use std::path::Path;
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};

use egui_pop::api::bus::bus_contract::PointId;
use egui_pop::api::bus::bus_contract::{
    self, Bus, HookError, ListenOpts, Point, PointDecl, Refused,
};
use helios_library::import::Card;
use serde_json::{Value, json};

/// The hook point a backup job's end passes through, and the journal's
/// event once it did.
pub const ENDED: &str = "helios.backup.ended";
/// The hook point the cards found safe to format pass through, and the
/// journal's event once they did.
pub const SAFE: &str = "helios.card.safe";
/// The owner of storage's listeners of both points.
pub const BUS_OWNER: &str = "helios.storage";
/// The owner of the health check's listener ([`HEALTH`]).
pub const HEALTH_OWNER: &str = "helios.health";

/// Each photo's backups read again: its badges and `backup:` filters.
pub const BACKUPS: &str = "helios.storage.backups";
/// The cards still inserted checked again, off the frame.
pub const SAFETY: &str = "helios.cards.safety";
/// The cards safe to format said in a toast and in the Log.
pub const TOAST: &str = "helios.cards.toast";
/// The Storage page's reads made out of date: its card rows read again.
pub const STORAGE: &str = "helios.storage.cards";
/// The health check's "Cards safe to format" run again.
pub const HEALTH: &str = "helios.health.cards";

/// The listeners of [`BackupEnded`], each with the one it runs after:
/// the order they run in.
pub const ORDER: &[(&str, Option<&str>)] = &[(BACKUPS, None), (SAFETY, Some(BACKUPS))];

/// The listeners of [`CardSafe`], in order.
pub const SAFE_ORDER: &[(&str, Option<&str>)] = &[
    (TOAST, None),
    (STORAGE, Some(TOAST)),
    (HEALTH, Some(STORAGE)),
];

/// A backup job's end, as [`BackupEnded`]'s listeners read it.
#[derive(Clone, Debug, Default)]
pub struct BackupEnd {}

/// The cards found safe to format, as [`CardSafe`]'s listeners read
/// them: as the list holds them now.
#[derive(Clone, Debug, Default)]
pub struct SafeCards {
    pub cards: Vec<Card>,
}

/// What the points' listeners did, in order.
#[derive(Clone, Debug, Default)]
pub struct Reactions {
    /// The listeners that ran, in their order.
    pub ran: Vec<String>,
}

/// The hook point a backup job's end passes through.
pub struct BackupEnded;

impl Point for BackupEnded {
    const ID: PointId = PointId::new(ENDED);
    type Input = BackupEnd;
    type Value = Reactions;
}

/// The hook point the cards found safe to format pass through.
pub struct CardSafe;

impl Point for CardSafe {
    const ID: PointId = PointId::new(SAFE);
    type Input = SafeCards;
    type Value = Reactions;
}

/// The backup points, this crate's contract with the application's bus:
/// declared by [`declare`].
pub const POINTS: &[PointDecl] = &[
    PointDecl::of::<BackupEnded>("A backup job ended: the badges and the cards checked again"),
    PointDecl::of::<CardSafe>("Cards inserted are safe to format: who says it"),
];

/// Declares [`POINTS`] on `bus` (once; a second desktop on the same bus
/// finds them there).
pub fn declare(bus: &dyn Bus) {
    for refused in bus_contract::declare_all(bus, POINTS) {
        if !matches!(refused, Refused::AlreadyDeclared { .. }) {
            log::error!(target: "helios", "backups: {refused}");
        }
    }
}

/// Listener `id`'s options: its owner (the health check's for
/// [`HEALTH`], storage's for the others) and the one it runs after.
fn opts(id: &str, after: Option<&str>) -> ListenOpts {
    let owner = if id == HEALTH {
        HEALTH_OWNER
    } else {
        BUS_OWNER
    };
    let opts = ListenOpts::new().owner(owner);
    match after {
        Some(after) => opts.after(after),
        None => opts,
    }
}

/// Listener `id`'s entry in `order`: its id, and the one it runs after.
fn place<'o>(
    order: &'o [(&'static str, Option<&'static str>)],
    id: &str,
) -> Option<&'o (&'static str, Option<&'static str>)> {
    order.iter().find(|(listener, _)| *listener == id)
}

/// Registers `run` as the listener `id` of [`BackupEnded`] (one of
/// [`ORDER`]'s) on `bus`, in its declared place: a local listener bound
/// to the calling thread (the interface's). One there already under `id`
/// is replaced.
///
/// # Errors
/// `id` is not in [`ORDER`], or the bus refused it.
pub fn listen_ended(
    bus: &dyn Bus,
    id: &'static str,
    run: impl Fn(&BackupEnd, &mut Reactions) + 'static,
) -> Result<(), String> {
    let Some(&(_, after)) = place(ORDER, id) else {
        return Err(format!("`{id}` is not a listener of {ENDED}"));
    };
    declare(bus);
    bus.unlisten(ENDED, id);
    let run = Rc::new(run);
    bus.listen_local::<BackupEnded>(id, opts(id, after), move |end, reactions| {
        run(end, reactions);
        reactions.ran.push(id.to_owned());
        Ok::<(), HookError>(())
    })
    .map_err(|e| e.to_string())
}

/// Registers `run` as the listener `id` of [`CardSafe`] (one of
/// [`SAFE_ORDER`]'s) on `bus`, in its declared place: a local listener
/// bound to the calling thread (the interface's). One there already under
/// `id` is replaced.
///
/// # Errors
/// `id` is not in [`SAFE_ORDER`], or the bus refused it.
pub fn listen_local(
    bus: &dyn Bus,
    id: &'static str,
    run: impl Fn(&SafeCards, &mut Reactions) + 'static,
) -> Result<(), String> {
    let Some(&(_, after)) = place(SAFE_ORDER, id) else {
        return Err(format!("`{id}` is not a listener of {SAFE}"));
    };
    declare(bus);
    bus.unlisten(SAFE, id);
    let run = Rc::new(run);
    bus.listen_local::<CardSafe>(id, opts(id, after), move |safe, reactions| {
        run(safe, reactions);
        reactions.ran.push(id.to_owned());
        Ok::<(), HookError>(())
    })
    .map_err(|e| e.to_string())
}

/// Passes a backup's end through [`BackupEnded`] on `bus`: what the
/// listeners did, in order.
pub fn pass_ended(bus: &dyn Bus, end: BackupEnd) -> Reactions {
    let altered = bus.alter::<BackupEnded>(end, Reactions::default());
    for failure in &altered.failures {
        log::warn!(target: "helios", "{ENDED}: {failure}");
    }
    altered.value
}

/// Passes the cards found safe through [`CardSafe`] on `bus`: what the
/// listeners did, in order.
pub fn pass_safe(bus: &dyn Bus, safe: SafeCards) -> Reactions {
    let altered = bus.alter::<CardSafe>(safe, Reactions::default());
    for failure in &altered.failures {
        log::warn!(target: "helios", "{SAFE}: {failure}");
    }
    altered.value
}

/// The order the backup points' listeners run in on `bus`.
pub fn order(bus: &dyn Bus) -> String {
    bus_contract::order_dump(bus, &[ENDED, SAFE])
}

/// A card's state as the list holds it, for whoever shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardState {
    /// Every photo on it has its backups: it may be formatted.
    Safe,
    /// Some photos are short of their backups: keep it.
    Waiting,
}

impl CardState {
    /// Its name, as the Storage page's rows say it.
    pub fn name(self) -> &'static str {
        match self {
            CardState::Safe => "safe",
            CardState::Waiting => "waiting",
        }
    }
}

/// The cards an import left short of their backups, while inserted: the
/// one list the shell, the Storage page's command and the health check's
/// thread read. A card taken out is no longer followed; a card found safe
/// stays, said safe, until taken out.
#[derive(Clone, Debug, Default)]
pub struct CardList(Arc<Mutex<Vec<Card>>>);

impl CardList {
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Card>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The cards an import read (`cards`): those short of their backups
    /// wait for a backup job to end.
    pub fn wait_for_backups(&self, cards: &[Card]) {
        let mut list = self.lock();
        for card in cards {
            if card.safe_to_format || card.missing > 0 || card.short == 0 {
                continue;
            }
            list.retain(|c| c.path != card.path);
            list.push(card.clone());
        }
    }

    /// The cards still inserted (their folder still there), the others
    /// forgotten.
    pub fn inserted(&self) -> Vec<Card> {
        let mut list = self.lock();
        list.retain(|c| c.path.is_dir());
        list.clone()
    }

    /// The cards inserted that still wait for their backups.
    pub fn waiting(&self) -> Vec<Card> {
        self.inserted()
            .into_iter()
            .filter(|c| !c.safe_to_format)
            .collect()
    }

    /// The card at `path` checked again (`now`); whether it just became
    /// safe to format.
    pub fn checked(&self, path: &Path, now: Card) -> bool {
        let mut list = self.lock();
        let Some(card) = list.iter_mut().find(|c| c.path == path) else {
            return false;
        };
        let became = now.safe_to_format && !card.safe_to_format;
        *card = now;
        became
    }

    /// The state of the card mounted at `root`, when the list follows it.
    pub fn state_of(&self, root: &Path) -> Option<CardState> {
        self.lock()
            .iter()
            .find(|c| c.path == root && c.path.is_dir())
            .map(|c| {
                if c.safe_to_format {
                    CardState::Safe
                } else {
                    CardState::Waiting
                }
            })
    }
}

/// The journal's words for a pass: the listeners that ran, and more.
pub fn said(ran: &[String], more: Value) -> Value {
    let mut data = json!({ "ran": ran });
    if let (Some(data), Value::Object(more)) = (data.as_object_mut(), more) {
        data.extend(more);
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(dir: &Path, label: &str, short: u64) -> Card {
        let path = dir.join(label);
        std::fs::create_dir_all(&path).unwrap();
        Card {
            path,
            label: label.into(),
            photos: 1,
            missing: 0,
            want: 1,
            short,
            safe_to_format: short == 0,
            root: dir.join("Library"),
            files: Vec::new(),
        }
    }

    #[test]
    fn listeners_run_in_the_declared_order() {
        let bus = egui_pop::api::bus::egui_pop_hooks::Kernel::new();
        for (id, _) in ORDER.iter().rev() {
            listen_ended(&bus, id, |_, _| {}).unwrap();
        }
        for (id, _) in SAFE_ORDER.iter().rev() {
            listen_local(&bus, id, |_, _| {}).unwrap();
        }
        assert_eq!(
            pass_ended(&bus, BackupEnd::default()).ran,
            [BACKUPS, SAFETY]
        );
        assert_eq!(
            pass_safe(&bus, SafeCards::default()).ran,
            [TOAST, STORAGE, HEALTH]
        );
        assert!(listen_ended(&bus, TOAST, |_, _| {}).is_err());
        assert!(listen_local(&bus, BACKUPS, |_, _| {}).is_err());
    }

    /// One list: a card checked safe is said safe by every reader, stays
    /// until taken out, and is said to become safe once.
    #[test]
    fn a_card_checked_safe_is_safe_for_every_reader_once() {
        let dir = std::env::temp_dir().join(format!("helios-card-list-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let list = CardList::default();
        let (a, b) = (card(&dir, "A", 1), card(&dir, "B", 1));
        list.wait_for_backups(&[a.clone(), b.clone(), card(&dir, "C", 0)]);
        assert_eq!(list.waiting().len(), 2, "C is safe already: not followed");
        assert!(list.checked(&a.path, card(&dir, "A", 0)));
        assert!(!list.checked(&a.path, card(&dir, "A", 0)), "said once");
        assert!(!list.checked(&b.path, card(&dir, "B", 1)));
        assert_eq!(list.state_of(&a.path), Some(CardState::Safe));
        assert_eq!(list.state_of(&b.path), Some(CardState::Waiting));
        assert_eq!(list.waiting().len(), 1);
        std::fs::remove_dir_all(&a.path).unwrap();
        assert_eq!(list.state_of(&a.path), None, "taken out");
        assert_eq!(list.inserted().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
