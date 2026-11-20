//! What storage read, handed to whoever shows it through one point.
//!
//! Storage reads the volumes, the backup strategy and each photo's
//! backups through its link (the Storage page's reads). Once it has them,
//! it passes them through the hook point [`StorageRead`]
//! (`helios.storage.read`) instead of writing into the Library: the
//! Library's listener ([`LIBRARY`]) names the offline photos' volumes and
//! sets the backup badges and the `backup:` filters from what it reads.
//! Storage knows the point, not the Library.
//!
//! The listeners hold the interface's state (`Rc`): they are local
//! listeners ([`listen_local`]), bound to the interface's thread, where
//! storage passes the point.

use std::collections::HashMap;
use std::rc::Rc;

use egui_pop::api::bus::bus_contract::PointId;
use egui_pop::api::bus::bus_contract::{
    self, Bus, HookError, ListenOpts, Point, PointDecl, Refused,
};
use helios_storage::Volume;
use helios_storage::strategy::BackupStatus;

/// The hook point what storage read passes through.
pub const READ: &str = "helios.storage.read";

/// The Library's listener: the volumes it names, its backup badges and
/// filters.
pub const LIBRARY: &str = "helios.library.storage";

/// The listeners of [`StorageRead`], each with its owner and the one it
/// runs after: the order they run in.
pub const ORDER: &[(&str, &str, Option<&str>)] = &[(LIBRARY, "helios.library", None)];

/// The backups as storage read them: each photo's status, by id, and
/// what the strategy says of them.
#[derive(Clone, Debug)]
pub struct BackupsRead {
    /// Each photo's backup status against the strategy, by id.
    pub statuses: HashMap<String, BackupStatus>,
    /// Whether the badges show: a strategy is set.
    pub badges: bool,
    /// The strategy's rule over the volumes known.
    pub rule: helios_vault::Backups,
}

/// What storage read, as [`StorageRead`]'s listeners read it: the parts
/// read this time, the others `None`.
#[derive(Clone, Debug, Default)]
pub struct Readout {
    /// The volumes known, online ones first.
    pub volumes: Option<Vec<Volume>>,
    /// The photos' backups.
    pub backups: Option<BackupsRead>,
}

/// What the point's listeners did, in order.
#[derive(Clone, Debug, Default)]
pub struct Reactions {
    /// The listeners that ran, in their order.
    pub ran: Vec<String>,
}

/// The hook point what storage read passes through.
pub struct StorageRead;

impl Point for StorageRead {
    const ID: PointId = PointId::new(READ);
    type Input = Readout;
    type Value = Reactions;
}

/// The point, this module's contract with the application's bus:
/// declared by [`declare`].
pub const POINTS: &[PointDecl] = &[PointDecl::of::<StorageRead>(
    "Storage read the volumes or the photos' backups: who shows them",
)];

/// Declares [`POINTS`] on `bus` (once; a second desktop on the same bus
/// finds them there).
pub fn declare(bus: &dyn Bus) {
    for refused in bus_contract::declare_all(bus, POINTS) {
        if !matches!(refused, Refused::AlreadyDeclared { .. }) {
            log::error!(target: "helios", "storage: {refused}");
        }
    }
}

/// Registers `run` as the listener `id` of [`StorageRead`] (one of
/// [`ORDER`]'s) on `bus`, in its declared place: a local listener bound
/// to the calling thread (the interface's). One there already under `id`
/// is replaced.
///
/// # Errors
/// `id` is not in [`ORDER`], or the bus refused it.
pub fn listen_local(
    bus: &dyn Bus,
    id: &'static str,
    run: impl Fn(&Readout, &mut Reactions) + 'static,
) -> Result<(), String> {
    let Some(&(_, owner, after)) = ORDER.iter().find(|(listener, ..)| *listener == id) else {
        return Err(format!("`{id}` is not a listener of {READ}"));
    };
    declare(bus);
    bus.unlisten(READ, id);
    let opts = ListenOpts::new().owner(owner);
    let opts = match after {
        Some(after) => opts.after(after),
        None => opts,
    };
    let run = Rc::new(run);
    bus.listen_local::<StorageRead>(id, opts, move |read, reactions| {
        run(read, reactions);
        reactions.ran.push(id.to_owned());
        Ok::<(), HookError>(())
    })
    .map_err(|e| e.to_string())
}

/// Passes what storage read through [`StorageRead`] on `bus`: what the
/// listeners did, in order.
pub fn pass(bus: &dyn Bus, read: Readout) -> Reactions {
    let altered = bus.alter::<StorageRead>(read, Reactions::default());
    for failure in &altered.failures {
        log::warn!(target: "helios", "{READ}: {failure}");
    }
    altered.value
}

/// The order the point's listeners run in on `bus`.
pub fn order(bus: &dyn Bus) -> String {
    bus_contract::order_dump(bus, &[READ])
}
