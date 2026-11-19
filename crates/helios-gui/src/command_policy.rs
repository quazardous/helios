//! Helios's listener on egui-pop's `command.run`: the policy every
//! command run passes before its handler, whatever asked it (a key, the
//! palette, a menu, a toolbar, an agent, the node's window commands).
//!
//! `command.run` is egui-pop's Request point, declared fail-closed: a
//! listener refuses a run with `HookError::refuse(reason)`, the handler
//! does not run, the caller gets "refused by `helios.policy`: reason" and
//! the journal says `command.refused` with `by` and the reason. A listener
//! that fails refuses the run too.
//!
//! What the policy refuses today:
//! - **A dry run never acts** ([`helios_shell::policy::dry_run`]): a
//!   command that takes no `dry_run` (and is not read-only) would ignore
//!   it and run for real, so the dry run is refused. The node's window
//!   commands checked it already; here every path does.
//!
//! The candidates that will join it as rules (each when it has a need):
//! the origin refusals written inside commands (copying to other
//! applications, secrets, workshop confirmations), an agent's sandbox
//! (an origin denied a command), and a destructive command run without
//! its confirmation.

use egui_pop::api::bus::bus_contract::{Bus, HookError, ListenOpts, Point};
use egui_pop::api::bus::{CommandAsk, CommandRun};

/// The policy's listener on `command.run`.
pub const POLICY: &str = "helios.policy";
/// Its owner.
pub const BUS_OWNER: &str = "helios.policy";

/// Registers the policy ([`POLICY`]) on `command.run` on `bus`. One there
/// already is replaced.
///
/// # Errors
/// The bus refused it.
pub fn listen(bus: &dyn Bus) -> Result<(), String> {
    bus.unlisten(&CommandRun::ID, POLICY);
    bus.listen::<CommandRun>(POLICY, ListenOpts::new().owner(BUS_OWNER), |ask, _| {
        check(ask).map_err(HookError::refuse)
    })
    .map_err(|e| e.to_string())
}

/// Whether the run `ask` may go on, read against its command's spec in
/// the application asking. A run outside an application's ask, or of a
/// command it does not know, is left to egui-pop (which refuses an
/// unknown command before the point is asked).
///
/// # Errors
/// The reason the run is refused.
fn check(ask: &CommandAsk) -> Result<(), String> {
    let Some(app) = egui_pop::api::bus::asking() else {
        return Ok(());
    };
    let Some(spec) = app.command(&ask.command) else {
        return Ok(());
    };
    helios_shell::policy::dry_run(
        ask.command.as_str(),
        helios_shell::policy::declares_dry_run(&spec.args),
        spec.read_only,
        &ask.args,
    )
}

/// The order `command.run`'s listeners run in on `bus`.
pub fn order(bus: &dyn Bus) -> String {
    egui_pop::api::bus::bus_contract::order_dump(bus, &egui_pop::api::bus::COMMAND_POINTS)
}
