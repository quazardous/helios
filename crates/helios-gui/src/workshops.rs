//! Workshops, an experimental surface on macros (see
//! `docs/design/workshops.md`): a workshop is a macro whose file carries
//! an intent and relative targets, found by what one wants, applied after
//! the node the image is shown from and aimed on the open image.
//!
//! - Behind a switch, `interface.experimental.workshops` ([`SETTING`],
//!   user taste, off by default), read once when the desktop starts: on,
//!   the Workshops panel ([`PANEL`]) is declared
//!   ([`crate::helios_panels_with`]) and the `workshop.*` commands are
//!   registered in the "Experimental" category ([`CATEGORY`]); off,
//!   neither exists. Changing it takes effect at the next start.
//! - Off, a composite with a workshop block still loads and renders: it
//!   is the macro it is.
//! - The panel says it is experimental in its header.

use helios_core::Message;

/// The switch.
pub const SETTING: &str = "interface.experimental.workshops";

/// The panel's id.
pub const PANEL: &str = "helios.workshops";

/// The command-palette category of the experimental commands, so the
/// palette shows the word.
pub const CATEGORY: &str = "Experimental";

/// Whether workshops are on: the user's settings layer (its TOML text;
/// none: the default), then the run layer's `KEY=VALUE` pairs (the
/// desktop's `--set`).
pub fn enabled(user_layer: Option<&str>, run: &[(String, String)]) -> Result<bool, String> {
    let mut config = helios_config::Config::new(helios_config::Schema::core());
    if let Some(text) = user_layer {
        config
            .load_str(helios_config::Layer::User, text)
            .map_err(|problems| {
                problems
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; ")
            })?;
    }
    for (key, value) in run {
        // The run layer names any setting; only the ones known matter here.
        if config.schema().get(key).is_some() {
            config
                .set_str(helios_config::Layer::Run, key, value)
                .map_err(|p| format!("--set {key}={value}: {p}"))?;
        }
    }
    Ok(config.toggle(SETTING))
}

/// The `--set KEY=VALUE` pairs of a launch's arguments: one invocation's
/// settings (the run layer).
pub fn run_layer(args: &[String]) -> Result<Vec<(String, String)>, String> {
    let mut sets = Vec::new();
    let mut args = args.iter();
    while let Some(a) = args.next() {
        if a == SET_FLAG {
            let pair = args
                .next()
                .ok_or_else(|| format!("{SET_FLAG} takes KEY=VALUE"))?;
            let (key, value) = pair
                .split_once('=')
                .ok_or_else(|| format!("{SET_FLAG} {pair}: not KEY=VALUE"))?;
            sets.push((key.trim().to_string(), value.trim().to_string()));
        }
    }
    Ok(sets)
}

/// The flag giving one setting for this run (`--set KEY=VALUE`).
pub const SET_FLAG: &str = "--set";

/// The panel, empty for now: its experimental header.
pub fn panel_ui(ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let text = |m: Message| crate::i18n::text(&ctx, m);
    ui.weak(text(Message::new(
        "gui-workshops-experimental",
        "Experimental: may change; what it saves stays a macro",
    )));
    ui.separator();
    ui.weak(text(Message::new(
        "gui-workshops-none",
        "No workshops in this workspace yet",
    )));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_by_default_on_from_the_user_layer_or_the_run() {
        assert!(!enabled(None, &[]).unwrap());
        let on = "[interface.experimental]\nworkshops = true\n";
        assert!(enabled(Some(on), &[]).unwrap());
        let run = run_layer(&[
            "--set".into(),
            "interface.experimental.workshops=true".into(),
        ])
        .unwrap();
        assert!(enabled(None, &run).unwrap());
        // The run layer wins over the user's.
        let off = run_layer(&["--set".into(), format!("{SETTING}=false")]).unwrap();
        assert!(!enabled(Some(on), &off).unwrap());
        assert!(enabled(Some("[interface.experimental]\nworkshops = 3\n"), &[]).is_err());
        assert!(run_layer(&["--set".into()]).is_err());
        assert!(run_layer(&["--set".into(), "nokey".into()]).is_err());
    }
}
