//! The library changed, one signal.
//!
//! The vault says its library changed four ways (`helios.vault.library_changed`,
//! `helios.vault.locations_changed`, `helios.vault.changed_outside`,
//! `helios.vault.captures_set`), its stacks two (`helios.vault.stack_changed`,
//! `helios.library.decision`), and the desktop says it once an import ended
//! (`import.ended`'s `library.refill`) or a library step was undone or
//! redone ([`Publisher::changed`]); the node says an import through it
//! ended (`helios.library.imported`, its record in the import history).
//! [`crate::persist::Persist`] gathers
//! what one pass heard into one [`Change`] (its [`Kind`]s, and who said
//! it) and passes it once through the hook point [`LibraryChanged`], its
//! listeners in the order [`ORDER`] declares; once the Library read
//! again lands, the shell passes [`LibraryFilled`] ([`FILLED_ORDER`]).
//! Both are said on the journal (`library.changed`, `library.filled`)
//! with the listeners that ran (`ran`).
//!
//! The listeners of both points hold the interface's state (`Rc`): they
//! are local listeners ([`listen_changed`], [`listen_filled_local`]),
//! bound to the interface's thread, each doing its own part inside the
//! pass, `library.changed`'s only when the change concerns them
//! ([`wants`]). Their state is shared between the shell
//! ([`crate::shell::Shell::library_parts`]) and the persistence layer
//! (the reads off the interface's thread, [`crate::persist::Persist`]).

use std::collections::BTreeSet;
use std::sync::mpsc::{Receiver, Sender};

use egui_pop::api::bus::bus_contract::{
    self, Bus, HookError, ListenOpts, Point, PointDecl, Refused,
};
use serde_json::{Value, json};

/// The hook point a change of the library passes through, and the
/// journal's event once it did.
pub const CHANGED: &str = "helios.library.changed";
/// The hook point the Library read again passes through once shown,
/// and the journal's event once it did.
pub const FILLED: &str = "helios.library.filled";
/// The owner of the desktop's listeners of both points.
pub const BUS_OWNER: &str = "helios.library";

/// The volumes read again (the Storage page, the Library's volume names
/// and badges).
pub const VOLUMES: &str = "helios.library.volumes";
/// The Library read again, off the interface's thread (its cursor's
/// delta), and its thumbnails.
pub const FILL: &str = "helios.library.fill";
/// The albums and the quick collection read again.
pub const ALBUMS: &str = "helios.library.albums";
/// The stacks read again; after an import, the stacks it suggests to
/// review (when the workspace asks for them).
pub const STACKS: &str = "helios.library.stacks";
/// The Library's import sessions read again (its Imports section).
pub const IMPORTS: &str = "helios.library.imports";
/// The photos an import brought selected, once the Library holds them
/// (the same listener as `import.ended`'s, which names them).
pub const SELECT: &str = crate::import_end::SELECT;

/// The listeners of [`LibraryChanged`], each with the one it runs after:
/// the order they run in.
pub const ORDER: &[(&str, Option<&str>)] = &[
    (VOLUMES, None),
    (FILL, Some(VOLUMES)),
    (ALBUMS, Some(FILL)),
    (STACKS, Some(ALBUMS)),
    (IMPORTS, Some(STACKS)),
];

/// The listeners of [`LibraryFilled`], in order.
pub const FILLED_ORDER: &[(&str, Option<&str>)] = &[(SELECT, None)];

/// What changed in the library.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    /// Photos put in or taken out (an import, an undo or a redo, the
    /// trash, another client).
    Contents,
    /// Where the copies are, or their state (a scan, a disk plugged in
    /// or out, a file changed outside Helios).
    Locations,
    /// When photos were taken (their capture times set).
    Captures,
    /// The stacks, or the suggestions waiting.
    Stacks,
    /// An import recorded in the import history (any client's).
    Imports,
}

impl Kind {
    /// Its name, as the journal says it.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Contents => "contents",
            Kind::Locations => "locations",
            Kind::Captures => "captures",
            Kind::Stacks => "stacks",
            Kind::Imports => "imports",
        }
    }

    /// What a vault event of kind `event` says changed, if anything here.
    pub fn of_event(event: &str) -> Option<Kind> {
        match event {
            helios_vault::LIBRARY_CHANGED => Some(Kind::Contents),
            helios_vault::LOCATIONS_CHANGED | helios_vault::changed::CHANGED_OUTSIDE => {
                Some(Kind::Locations)
            }
            helios_vault::capture::CAPTURES_SET => Some(Kind::Captures),
            "helios.vault.stack_changed" | "helios.library.decision" => Some(Kind::Stacks),
            "helios.library.imported" => Some(Kind::Imports),
            _ => None,
        }
    }
}

/// Who said the library changed: the vault's events, an import's end,
/// a library step undone or redone.
pub const BY_VAULT: &str = "vault";
/// An import ended ([`crate::import_end::REFILL`]).
pub const BY_IMPORT: &str = "import";
/// A library step undone or redone.
pub const BY_HISTORY: &str = "history";

/// What one pass heard changed, as [`LibraryChanged`]'s listeners read
/// it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Change {
    pub kinds: BTreeSet<Kind>,
    /// Who said it ([`BY_VAULT`], [`BY_IMPORT`], [`BY_HISTORY`]).
    pub by: BTreeSet<&'static str>,
}

impl Change {
    /// Adds `kind`, said by `by`.
    pub fn add(&mut self, kind: Kind, by: &'static str) {
        self.kinds.insert(kind);
        self.by.insert(by);
    }

    pub fn is_empty(&self) -> bool {
        self.kinds.is_empty()
    }

    /// As the journal says it.
    pub fn to_json(&self) -> Value {
        json!({
            "kinds": self.kinds.iter().map(|k| k.name()).collect::<Vec<_>>(),
            "by": self.by,
        })
    }
}

/// Whether listener `id` of [`LibraryChanged`] has a part in `change`.
pub fn wants(id: &str, change: &Change) -> bool {
    let any = |kinds: &[Kind]| kinds.iter().any(|k| change.kinds.contains(k));
    match id {
        VOLUMES | FILL => any(&[Kind::Contents, Kind::Locations, Kind::Captures]),
        ALBUMS => any(&[Kind::Contents]),
        STACKS => any(&[Kind::Stacks]) || change.by.contains(BY_IMPORT),
        IMPORTS => any(&[Kind::Imports]),
        _ => false,
    }
}

/// The Library read again and shown, as [`LibraryFilled`]'s listeners
/// read it.
#[derive(Clone, Debug, Default)]
pub struct Filled {
    /// How many photos it shows.
    pub photos: usize,
}

/// What the points' listeners did, in order.
#[derive(Clone, Debug, Default)]
pub struct Reactions {
    /// The listeners that ran, in their order.
    pub ran: Vec<String>,
}

/// The hook point a change of the library passes through.
pub struct LibraryChanged;

impl Point for LibraryChanged {
    const ID: &'static str = CHANGED;
    type Input = Change;
    type Value = Reactions;
}

/// The hook point the Library read again passes through once shown.
pub struct LibraryFilled;

impl Point for LibraryFilled {
    const ID: &'static str = FILLED;
    type Input = Filled;
    type Value = Reactions;
}

/// The library's points, this crate's contract with the application's
/// bus: declared by [`declare`].
pub const POINTS: &[PointDecl] = &[
    PointDecl::of::<LibraryChanged>("The library changed: what the desktop reads again")
        .budget(crate::INTERFACE_PASS_BUDGET),
    PointDecl::of::<LibraryFilled>("The Library read again is shown: what follows it"),
];

/// Declares [`POINTS`] on `bus` (once; a second desktop on the same bus
/// finds them there).
pub fn declare(bus: &dyn Bus) {
    for refused in bus_contract::declare_all(bus, POINTS) {
        if !matches!(refused, Refused::AlreadyDeclared { .. }) {
            log::error!(target: "helios", "library: {refused}");
        }
    }
}

fn opts(after: Option<&str>) -> ListenOpts {
    let opts = ListenOpts::new().owner(BUS_OWNER);
    match after {
        Some(after) => opts.after(after),
        None => opts,
    }
}

/// Registers the listener `id` of [`LibraryChanged`] (one of [`ORDER`]'s)
/// on `bus`, in its declared place: a local listener bound to the calling
/// thread (the interface's: its state is `Rc`), doing its part (`run`)
/// inside the pass when the change concerns it ([`wants`]). One there
/// already under `id` is replaced.
///
/// # Errors
/// `id` is not in [`ORDER`], or the bus refused it.
pub fn listen_changed(
    bus: &dyn Bus,
    id: &'static str,
    run: impl Fn(&Change) + 'static,
) -> Result<(), String> {
    let Some((_, after)) = ORDER.iter().find(|(listener, _)| *listener == id) else {
        return Err(format!("`{id}` is not a listener of {CHANGED}"));
    };
    declare(bus);
    bus.unlisten(CHANGED, id);
    bus.listen_local::<LibraryChanged>(id, opts(*after), move |change, reactions| {
        if wants(id, change) {
            run(change);
            reactions.ran.push(id.to_owned());
        }
        Ok::<(), HookError>(())
    })
    .map_err(|e| e.to_string())
}

/// Registers the listener `id` of [`LibraryFilled`] (one of
/// [`FILLED_ORDER`]'s) on `bus`, in its declared place; its part is done
/// by the one passing the point. One there already under `id` is
/// replaced.
///
/// # Errors
/// `id` is not in [`FILLED_ORDER`], or the bus refused it.
pub fn listen_filled(bus: &dyn Bus, id: &'static str) -> Result<(), String> {
    let Some((_, after)) = FILLED_ORDER.iter().find(|(listener, _)| *listener == id) else {
        return Err(format!("`{id}` is not a listener of {FILLED}"));
    };
    declare(bus);
    bus.unlisten(FILLED, id);
    bus.listen::<LibraryFilled>(id, opts(*after), move |_, reactions| {
        reactions.ran.push(id.to_owned());
        Ok::<(), HookError>(())
    })
    .map_err(|e| e.to_string())
}

/// [`listen_filled`] for a listener holding the interface's state (not
/// `Send + Sync`): a local listener bound to the calling thread (the
/// interface's), in the same declared place, doing its part (`run`)
/// inside the pass when the Library is shown on that thread.
///
/// # Errors
/// `id` is not in [`FILLED_ORDER`], or the bus refused it.
pub fn listen_filled_local(
    bus: &dyn Bus,
    id: &'static str,
    run: impl Fn(&Filled) + 'static,
) -> Result<(), String> {
    let Some((_, after)) = FILLED_ORDER.iter().find(|(listener, _)| *listener == id) else {
        return Err(format!("`{id}` is not a listener of {FILLED}"));
    };
    declare(bus);
    bus.unlisten(FILLED, id);
    bus.listen_local::<LibraryFilled>(id, opts(*after), move |filled, reactions| {
        run(filled);
        reactions.ran.push(id.to_owned());
        Ok::<(), HookError>(())
    })
    .map_err(|e| e.to_string())
}

/// Passes `change` through [`LibraryChanged`] on `bus`: the listeners
/// that ran, in order.
pub fn pass_changed(bus: &dyn Bus, change: Change) -> Vec<String> {
    let altered = bus.alter::<LibraryChanged>(change, Reactions::default());
    for failure in &altered.failures {
        log::warn!(target: "helios", "{CHANGED}: {failure}");
    }
    altered.value.ran
}

/// Passes `filled` through [`LibraryFilled`] on `bus`: the listeners
/// that ran, in order.
pub fn pass_filled(bus: &dyn Bus, filled: Filled) -> Vec<String> {
    let altered = bus.alter::<LibraryFilled>(filled, Reactions::default());
    for failure in &altered.failures {
        log::warn!(target: "helios", "{FILLED}: {failure}");
    }
    altered.value.ran
}

/// The order the library points' listeners run in on `bus`.
pub fn order(bus: &dyn Bus) -> String {
    bus_contract::order_dump(bus, &[CHANGED, FILLED])
}

/// Where the desktop says the library changed, from any thread: heard
/// with the vault's events at the next pass, and passed with them as one
/// change.
#[derive(Clone)]
pub struct Publisher {
    said: Sender<(Kind, &'static str)>,
    wake: Option<egui::Context>,
}

impl Publisher {
    /// A publisher and where what it says is heard.
    pub fn channel() -> (Publisher, Receiver<(Kind, &'static str)>) {
        let (said, heard) = std::sync::mpsc::channel();
        (Publisher { said, wake: None }, heard)
    }

    /// `ctx` woken once a change is said.
    #[must_use]
    pub fn waking(mut self, ctx: egui::Context) -> Publisher {
        self.wake = Some(ctx);
        self
    }

    /// The library changed (`kind`), said by `by`.
    pub fn changed(&self, kind: Kind, by: &'static str) {
        let _ = self.said.send((kind, by));
        if let Some(ctx) = &self.wake {
            ctx.request_repaint();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listeners_run_in_the_declared_order_and_only_when_concerned() {
        let bus = egui_pop::api::bus::egui_pop_hooks::Kernel::new();
        for (id, _) in ORDER.iter().rev() {
            listen_changed(&bus, id, |_| {}).unwrap();
        }
        let mut change = Change::default();
        change.add(Kind::Contents, BY_IMPORT);
        assert_eq!(pass_changed(&bus, change), [VOLUMES, FILL, ALBUMS, STACKS]);
        let mut change = Change::default();
        change.add(Kind::Locations, BY_VAULT);
        assert_eq!(pass_changed(&bus, change), [VOLUMES, FILL]);
        let mut change = Change::default();
        change.add(Kind::Stacks, BY_VAULT);
        assert_eq!(pass_changed(&bus, change), [STACKS]);
        let mut change = Change::default();
        change.add(Kind::Contents, BY_HISTORY);
        assert_eq!(pass_changed(&bus, change), [VOLUMES, FILL, ALBUMS]);
        let mut change = Change::default();
        change.add(Kind::Imports, BY_VAULT);
        assert_eq!(pass_changed(&bus, change), [IMPORTS]);
    }

    /// Each listener does its own part inside the pass, in the declared
    /// order: done when `pass_changed` returns, nothing left to the one
    /// passing the point.
    #[test]
    fn each_listener_does_its_part_inside_the_pass() {
        use std::cell::RefCell;
        use std::rc::Rc;
        let bus = egui_pop::api::bus::egui_pop_hooks::Kernel::new();
        let done: Rc<RefCell<Vec<String>>> = Rc::default();
        for (id, _) in ORDER {
            let done = done.clone();
            listen_changed(&bus, id, move |change| {
                done.borrow_mut()
                    .push(format!("{id} {}", change.to_json()["kinds"]));
            })
            .unwrap();
        }
        let mut change = Change::default();
        change.add(Kind::Locations, BY_VAULT);
        let ran = pass_changed(&bus, change);
        assert_eq!(ran, [VOLUMES, FILL]);
        assert_eq!(
            *done.borrow(),
            [
                "helios.library.volumes [\"locations\"]",
                "helios.library.fill [\"locations\"]"
            ]
        );
        assert!(
            order(&bus).contains(
                "helios.library.fill (owner helios.library): local; after helios.library.volumes"
            ),
            "{}",
            order(&bus)
        );
    }

    #[test]
    fn a_listener_not_declared_is_refused() {
        let bus = egui_pop::api::bus::egui_pop_hooks::Kernel::new();
        assert!(listen_changed(&bus, "nobody.knows", |_| {}).is_err());
        assert!(listen_filled(&bus, FILL).is_err());
    }

    #[test]
    fn the_vault_events_say_their_kind() {
        assert_eq!(
            Kind::of_event(helios_vault::LIBRARY_CHANGED),
            Some(Kind::Contents)
        );
        assert_eq!(
            Kind::of_event(helios_vault::changed::CHANGED_OUTSIDE),
            Some(Kind::Locations)
        );
        assert_eq!(
            Kind::of_event(helios_vault::capture::CAPTURES_SET),
            Some(Kind::Captures)
        );
        assert_eq!(
            Kind::of_event("helios.vault.stack_changed"),
            Some(Kind::Stacks)
        );
        assert_eq!(
            Kind::of_event("helios.library.imported"),
            Some(Kind::Imports)
        );
        assert_eq!(Kind::of_event("helios.vault.field_changed"), None);
    }
}
