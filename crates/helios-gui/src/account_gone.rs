//! An import account signed out or removed, said once on the bus.
//!
//! Signing an account out (`import.signOut`, Settings > Accounts'
//! "Disconnect, keep its sources") and removing it (`import.disconnect`
//! with no source kept) are requests the shell checks, then passes once
//! through a hook point: [`SignedOut`] (`account.signedOut`) or
//! [`Removed`] (`account.removed`). Their listeners run in the order
//! [`ORDER`] declares, the same for both:
//!
//! 1. `import.cancel-picking`: a choice of photos under way on the
//!    account (the service's page, Google Photos' picker) is cancelled,
//!    its sign-in running given up, its photos chosen and not imported
//!    given up. First: a picking left running on an account signed out
//!    would find its token gone and write the account back as "needs
//!    attention", or run on half an hour on a removed one.
//! 2. `accounts.credentials`: its token revoked where the service can,
//!    its secrets forgotten from the keyring, the account written "not
//!    connected" (signed out) or forgotten whole (removed). Before the
//!    readers below: they read the account as the store holds it now.
//! 3. `import.sources`: the import window's Sources column and Detail
//!    column follow in the same pass; a removed account's saved sources
//!    are forgotten (steps of the window's Undo).
//! 4. `storage.owners`: the Storage page's volumes, named by the
//!    accounts behind them, read again.
//! 5. `settings.accounts`: Settings > Accounts' rows read again.
//! 6. `health.accounts`: the checks handed the accounts (the plugins'
//!    and their own) run again.
//!
//! The pass is then said on the journal under the point's name with the
//! listeners that ran (`ran`). The listeners hold the interface's state
//! (`Rc`): they are local listeners, bound to the interface's thread,
//! where the shell passes both points.

use std::rc::Rc;

use egui_pop::api::bus::bus_contract::{
    self, Bus, HookError, ListenOpts, Point, PointDecl, Refused,
};

/// The hook point an account signed out passes through, and the
/// journal's event once it did.
pub const SIGNED_OUT: &str = "account.signedOut";
/// The hook point an account removed passes through, and the journal's
/// event once it did.
pub const REMOVED: &str = "account.removed";

/// A choice of photos under way on the account cancelled.
pub const CANCEL_PICKING: &str = "import.cancel-picking";
/// The token revoked, the keyring's secrets forgotten, the account
/// written.
pub const CREDENTIALS: &str = "accounts.credentials";
/// The import window's Sources and Detail columns.
pub const SOURCES: &str = "import.sources";
/// The Storage page's volumes, named by their accounts.
pub const STORAGE: &str = "storage.owners";
/// Settings > Accounts' rows.
pub const SETTINGS: &str = "settings.accounts";
/// The health checks handed the accounts.
pub const HEALTH: &str = "health.accounts";

/// The listeners of both points: each with its owner and the one it runs
/// after; the order they run in.
pub const ORDER: &[(&str, &str, Option<&str>)] = &[
    (CANCEL_PICKING, "helios.import", None),
    (CREDENTIALS, "helios.accounts", Some(CANCEL_PICKING)),
    (SOURCES, "helios.import", Some(CREDENTIALS)),
    (STORAGE, "helios.storage", Some(SOURCES)),
    (SETTINGS, "helios.settings", Some(STORAGE)),
    (HEALTH, "helios.health", Some(SETTINGS)),
];

/// How the account goes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum How {
    /// Signed out: listed still, "not connected", its sources kept.
    #[default]
    SignedOut,
    /// Removed: forgotten, with its saved sources.
    Removed,
}

/// The account that goes, as the listeners read it.
#[derive(Clone, Debug, Default)]
pub struct Gone {
    /// Its id.
    pub account: String,
    pub how: How,
}

/// What the points' listeners did, in order.
#[derive(Clone, Debug, Default)]
pub struct Reactions {
    /// The listeners that ran, in their order.
    pub ran: Vec<String>,
    /// What failed, said to whoever asked (a keyring that refused).
    pub failed: Vec<String>,
}

/// The hook point an account signed out passes through.
pub struct SignedOut;

impl Point for SignedOut {
    const ID: &'static str = SIGNED_OUT;
    type Input = Gone;
    type Value = Reactions;
}

/// The hook point an account removed passes through.
pub struct Removed;

impl Point for Removed {
    const ID: &'static str = REMOVED;
    type Input = Gone;
    type Value = Reactions;
}

/// The account points, this crate's contract with the application's
/// bus: declared by [`declare`].
pub const POINTS: &[PointDecl] = &[
    PointDecl::of::<SignedOut>(
        "An import account signed out: its picking cancelled, its secrets forgotten, who shows it",
    ),
    PointDecl::of::<Removed>(
        "An import account removed: its picking cancelled, its secrets and sources forgotten, who shows it",
    ),
];

/// Declares [`POINTS`] on `bus` (once; a second desktop on the same bus
/// finds them there).
pub fn declare(bus: &dyn Bus) {
    for refused in bus_contract::declare_all(bus, POINTS) {
        if !matches!(refused, Refused::AlreadyDeclared { .. }) {
            log::error!(target: "helios", "accounts: {refused}");
        }
    }
}

/// Registers `run` as the listener `id` (one of [`ORDER`]'s) of both
/// points on `bus`, in its declared place: a local listener bound to the
/// calling thread (the interface's). One there already under `id` is
/// replaced.
///
/// # Errors
/// `id` is not in [`ORDER`], or the bus refused it.
pub fn listen_local(
    bus: &dyn Bus,
    id: &'static str,
    run: impl Fn(&Gone, &mut Reactions) + 'static,
) -> Result<(), String> {
    let Some(&(_, owner, after)) = ORDER.iter().find(|(listener, _, _)| *listener == id) else {
        return Err(format!("`{id}` is not a listener of {SIGNED_OUT}"));
    };
    declare(bus);
    let opts = || {
        let opts = ListenOpts::new().owner(owner);
        match after {
            Some(after) => opts.after(after),
            None => opts,
        }
    };
    let run = Rc::new(run);
    bus.unlisten(SIGNED_OUT, id);
    let then = run.clone();
    bus.listen_local::<SignedOut>(id, opts(), move |gone, reactions| {
        then(gone, reactions);
        reactions.ran.push(id.to_owned());
        Ok::<(), HookError>(())
    })
    .map_err(|e| e.to_string())?;
    bus.unlisten(REMOVED, id);
    bus.listen_local::<Removed>(id, opts(), move |gone, reactions| {
        run(gone, reactions);
        reactions.ran.push(id.to_owned());
        Ok::<(), HookError>(())
    })
    .map_err(|e| e.to_string())
}

/// Passes `gone` through its point on `bus`: what the listeners did, in
/// order.
pub fn pass(bus: &dyn Bus, gone: Gone) -> Reactions {
    let (name, altered) = match gone.how {
        How::SignedOut => (
            SIGNED_OUT,
            bus.alter::<SignedOut>(gone, Reactions::default()),
        ),
        How::Removed => (REMOVED, bus.alter::<Removed>(gone, Reactions::default())),
    };
    let mut reactions = altered.value;
    for failure in &altered.failures {
        log::warn!(target: "helios", "{name}: {failure}");
        reactions.failed.push(failure.to_string());
    }
    reactions
}

/// The point's name `how` passes through, and the journal's event.
pub fn name(how: How) -> &'static str {
    match how {
        How::SignedOut => SIGNED_OUT,
        How::Removed => REMOVED,
    }
}

/// The order the account points' listeners run in on `bus`.
pub fn order(bus: &dyn Bus) -> String {
    bus_contract::order_dump(bus, &[SIGNED_OUT, REMOVED])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listeners_run_in_the_declared_order_for_both_points() {
        let bus = egui_pop::api::bus::egui_pop_hooks::Kernel::new();
        for (id, _, _) in ORDER.iter().rev() {
            listen_local(&bus, id, |_, _| {}).unwrap();
        }
        let ids: Vec<&str> = ORDER.iter().map(|(id, _, _)| *id).collect();
        for how in [How::SignedOut, How::Removed] {
            let gone = Gone {
                account: "a".into(),
                how,
            };
            assert_eq!(pass(&bus, gone).ran, ids, "{}", name(how));
        }
        assert!(listen_local(&bus, "cards.toast", |_, _| {}).is_err());
    }

    /// Each listener reads the account and how it goes.
    #[test]
    fn listeners_read_the_account_and_how_it_goes() {
        let bus = egui_pop::api::bus::egui_pop_hooks::Kernel::new();
        listen_local(&bus, CANCEL_PICKING, |gone, reactions| {
            reactions
                .failed
                .push(format!("{} {:?}", gone.account, gone.how));
        })
        .unwrap();
        let gone = Gone {
            account: "acc-1".into(),
            how: How::Removed,
        };
        assert_eq!(pass(&bus, gone).failed, ["acc-1 Removed"]);
    }
}
