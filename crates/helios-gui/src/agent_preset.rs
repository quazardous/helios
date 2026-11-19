//! What agents on the socket may do when a run asks for more than the
//! defaults: the scenario preset, for scenario and test runs on a
//! throwaway library (docs/gui.md, "Agents").
//!
//! Without a preset, egui-pop-agent's defaults hold: the bridge reads
//! only, an unpaired client reads, a paired agent (approved by the user)
//! edits, destructive commands asked first.
//!
//! `HELIOS_AGENT_PRESET=scenario` makes the bridge writable and gives
//! the scenario grant to paired agents and to the unpaired clients
//! scenario runners are (`examples/scenarios.rs`,
//! `egui_pop_test::scenario::live`, which do not say `hello` yet): every
//! command, value writes and `ui` actions, private data withheld.
//! `scenario+private` adds private data (paths, accounts, people,
//! places), which importing a folder or opening a file by its path needs.
//! Destructive commands are still held for the user's yes.
//!
//! `HELIOS_AGENT_WRITE=1` is the preset's deprecated alias, kept for one
//! release: it means `scenario+private`, what it allowed before.

use egui_pop_agent::{AgentGrants, Grant};

/// The environment variable naming the preset.
pub const PRESET: &str = "HELIOS_AGENT_PRESET";
/// Deprecated: `HELIOS_AGENT_WRITE=1` is `HELIOS_AGENT_PRESET=scenario+private`.
pub const AGENT_WRITE: &str = "HELIOS_AGENT_WRITE";

/// A preset of the agents' grants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    /// Scenario and test runs: write, act, private data only when asked.
    Scenario { private: bool },
}

impl Preset {
    /// The preset a name means: `scenario` or `scenario+private`.
    pub fn parse(name: &str) -> Result<Self, String> {
        match name.trim() {
            "scenario" => Ok(Self::Scenario { private: false }),
            "scenario+private" => Ok(Self::Scenario { private: true }),
            other => Err(format!(
                "{PRESET}={other}: unknown preset (scenario, scenario+private)"
            )),
        }
    }

    /// The preset the environment asks for, if any, with a warning to
    /// log when it is asked the deprecated way or wrongly.
    pub fn from_env(env: impl Fn(&str) -> Option<String>) -> (Option<Self>, Option<String>) {
        if let Some(name) = env(PRESET).filter(|n| !n.trim().is_empty()) {
            return match Self::parse(&name) {
                Ok(p) => (Some(p), None),
                Err(e) => (None, Some(e)),
            };
        }
        if env(AGENT_WRITE).is_some_and(|v| v == "1") {
            return (
                Some(Self::Scenario { private: true }),
                Some(format!(
                    "{AGENT_WRITE}=1 is deprecated: use {PRESET}=scenario+private \
                     (or scenario, without private data)"
                )),
            );
        }
        (None, None)
    }

    /// The grant each agent gets.
    pub fn grant(self) -> Grant {
        match self {
            Self::Scenario { private } => Grant::full().private(private),
        }
    }

    /// The grants: paired agents and unpaired clients alike.
    pub fn grants(self) -> AgentGrants {
        AgentGrants::new()
            .unpaired(self.grant())
            .paired(self.grant())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |n| {
            vars.iter()
                .find(|(k, _)| *k == n)
                .map(|(_, v)| (*v).to_string())
        }
    }

    #[test]
    fn the_preset_is_read_from_its_variable_and_the_old_switch_is_its_alias() {
        assert_eq!(
            Preset::from_env(env(&[(PRESET, "scenario")])),
            (Some(Preset::Scenario { private: false }), None)
        );
        assert_eq!(
            Preset::from_env(env(&[(PRESET, "scenario+private")])).0,
            Some(Preset::Scenario { private: true })
        );
        let (p, warning) = Preset::from_env(env(&[(AGENT_WRITE, "1")]));
        assert_eq!(p, Some(Preset::Scenario { private: true }));
        assert!(warning.unwrap().contains("deprecated"));
        assert_eq!(Preset::from_env(env(&[(AGENT_WRITE, "yes")])), (None, None));
        assert_eq!(Preset::from_env(env(&[])), (None, None));
        let (p, error) = Preset::from_env(env(&[(PRESET, "admin")]));
        assert_eq!(p, None);
        assert!(error.unwrap().contains("unknown preset"));
        // The preset wins over the alias.
        assert_eq!(
            Preset::from_env(env(&[(PRESET, "scenario"), (AGENT_WRITE, "1")])).0,
            Some(Preset::Scenario { private: false })
        );
    }

    #[test]
    fn the_scenario_grant_writes_and_withholds_private_data_unless_asked() {
        let g = Preset::Scenario { private: false }.grants();
        assert_eq!(g.unpaired, g.paired);
        assert!(!g.paired.read_only);
        assert!(g.paired.ui);
        assert!(!g.paired.private);
        assert!(Preset::Scenario { private: true }.grant().private);
        // Without a preset, egui-pop's defaults: unpaired clients read.
        assert!(AgentGrants::new().unpaired.read_only);
    }

    /// Helios's commands on egui-pop's agent socket, under the scenario
    /// preset: a client thread speaks JSON-RPC while the test steps
    /// frames, as the socket's own tests do.
    #[cfg(unix)]
    mod socket {
        use std::io::{BufRead as _, BufReader, Write as _};
        use std::os::unix::fs::PermissionsExt as _;
        use std::os::unix::net::UnixStream;
        use std::path::{Path, PathBuf};
        use std::sync::{Arc, Mutex};
        use std::time::{Duration, Instant};

        use egui_kittest::Harness;
        use egui_pop::api::dialogs::{Answer, DIALOGS, Dialog};
        use egui_pop::api::{App, Ctx, DefaultPlugins, Plugin, PluginError};
        use egui_pop::model::commands::Origin as PopOrigin;
        use egui_pop::model::ids::PluginId;
        use egui_pop_agent::{AgentGrantsExt as _, AgentSocket, AgentSocketExt as _};
        use helios_core::Bus;
        use helios_shell::commands::{CommandSpec as HeliosSpec, Registry};
        use serde_json::{Value as Json, json};

        use super::*;

        /// Declares Helios's commands as the shell does
        /// (`plugins::declared`).
        struct DeclaresHelios(Vec<HeliosSpec>);

        impl Plugin for DeclaresHelios {
            fn id(&self) -> PluginId {
                PluginId::from_static("test.helios")
            }

            fn connect(&mut self, ctx: &mut Ctx<'_>) -> Result<(), PluginError> {
                let d = crate::plugins::Declarations::default();
                for spec in &self.0 {
                    ctx.declare(crate::plugins::declared(spec, &d)?)?;
                }
                Ok(())
            }
        }

        /// What the handlers saw: each command run, with its arguments.
        type Ran = Arc<Mutex<Vec<(String, Json)>>>;

        /// An application with Helios's host and three of its commands:
        /// `library.trash` (destructive, with a schema), `vault.rate` (a
        /// write) and `library.folder` (private); its agent socket under
        /// the scenario preset, set in code, no variable.
        fn app(name: &str) -> (Harness<'static, App>, PathBuf, Ran) {
            let dir = std::env::temp_dir()
                .join(format!("helios-agent-preset-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
            let socket = dir.join("helios.sock");
            let registry = Arc::new(Mutex::new(Registry::new()));
            let ran: Ran = Arc::default();
            let specs = [
                HeliosSpec::new("library.trash", "Move to Trash", "Library"),
                HeliosSpec::new("vault.rate", "Rate", "Library"),
                HeliosSpec::new("library.folder", "Folder", "Library"),
            ];
            for spec in &specs {
                let ran = ran.clone();
                let id = spec.id.clone();
                registry
                    .lock()
                    .unwrap()
                    .register(spec.clone(), move |args, _| {
                        ran.lock().unwrap().push((id.clone(), args.clone()));
                        Ok(json!({ "ok": true }))
                    })
                    .unwrap();
            }
            let host = crate::host::HeliosHost::new(registry, Arc::new(Bus::new()), Arc::default());
            let preset = Preset::Scenario { private: false };
            let builder = App::builder()
                .host(host)
                .plugins(DefaultPlugins)
                .plugin(DeclaresHelios(specs.to_vec()))
                .debug_bridge(egui_pop::core::debug::BridgeMode::On)
                .agent_grants(preset.grants())
                .app_id("org.example.HeliosAgentPreset")
                .agent_socket(AgentSocket::at(&socket));
            let mut h = egui_pop_test::app(builder, egui::vec2(800.0, 600.0));
            h.step();
            (h, socket, ran)
        }

        /// A JSON-RPC client over the socket.
        struct Client {
            read: BufReader<UnixStream>,
            write: UnixStream,
            next: u64,
        }

        impl Client {
            fn connect(path: &Path) -> Self {
                let write = UnixStream::connect(path).expect("the socket answers");
                write
                    .set_read_timeout(Some(Duration::from_secs(20)))
                    .unwrap();
                Self {
                    read: BufReader::new(write.try_clone().unwrap()),
                    write,
                    next: 1,
                }
            }

            /// `commands.run` of `id`: the call's whole response.
            fn run(&mut self, id: &str, args: Json) -> Json {
                let n = self.next;
                self.next += 1;
                let params = json!({
                    "collector": "commands",
                    "action": "run",
                    "args": { "id": id, "args": args },
                    "deadline_ms": 20_000,
                });
                let req = json!({ "jsonrpc": "2.0", "id": n, "method": "call", "params": params });
                writeln!(self.write, "{req}").unwrap();
                let mut line = String::new();
                self.read.read_line(&mut line).expect("a response line");
                serde_json::from_str(&line).expect("JSON")
            }
        }

        /// Steps frames until `thread` ends.
        fn finish<T>(h: &mut Harness<'static, App>, thread: std::thread::JoinHandle<T>) -> T {
            let start = Instant::now();
            while !thread.is_finished() {
                assert!(
                    start.elapsed() < Duration::from_secs(30),
                    "the client did not finish"
                );
                h.step();
                std::thread::sleep(Duration::from_millis(2));
            }
            thread.join().expect("the client passes")
        }

        /// Steps frames until the socket's confirmation shows, then
        /// answers it as the user.
        fn user_confirms(h: &mut Harness<'static, App>, accept: bool) {
            let start = Instant::now();
            loop {
                h.step();
                let d = h.state().handle().service(&DIALOGS).unwrap();
                if let Some((_, Dialog::Confirm(c))) = d.current()
                    && c.command == egui_pop_agent::CMD_CONFIRM
                {
                    let answer = if accept {
                        Answer::Accept {
                            values: Default::default(),
                        }
                    } else {
                        Answer::Cancel
                    };
                    d.answer(answer, PopOrigin::User).unwrap();
                    return;
                }
                assert!(
                    start.elapsed() < Duration::from_secs(20),
                    "no confirmation showed"
                );
                std::thread::sleep(Duration::from_millis(2));
            }
        }

        fn names(ran: &Ran) -> Vec<String> {
            ran.lock().unwrap().iter().map(|(n, _)| n.clone()).collect()
        }

        fn refused_by(h: &Harness<'static, App>, by: &str) -> Vec<Json> {
            h.state()
                .handle()
                .events_since(0)
                .into_iter()
                .filter(|e| e.name == "command.refused" && e.data["by"] == by)
                .map(|e| e.data)
                .collect()
        }

        #[test]
        fn the_scenario_preset_writes_and_a_destructive_call_waits_for_the_user() {
            let (mut h, socket, ran) = app("destructive");
            let declared = h
                .state()
                .handle()
                .command(&egui_pop::model::ids::CommandId::from_static(
                    "library.trash",
                ))
                .unwrap();
            assert!(declared.destructive, "library.trash is destructive");

            // A write runs at once, with no variable set: the preset.
            let s = socket.clone();
            let client = std::thread::spawn(move || {
                Client::connect(&s).run("vault.rate", json!({ "stars": 3 }))
            });
            let rate = finish(&mut h, client);
            assert_eq!(rate["result"]["ok"], true, "{rate}");
            assert_eq!(names(&ran), ["vault.rate"]);

            // Private data is withheld unless the preset asks for it.
            let s = socket.clone();
            let client = std::thread::spawn(move || {
                Client::connect(&s).run("library.folder", json!({ "path": "/home/someone" }))
            });
            let folder = finish(&mut h, client);
            assert!(folder.get("error").is_some(), "{folder}");
            assert_eq!(names(&ran), ["vault.rate"]);

            // The destructive call is held until the user says yes.
            let s = socket.clone();
            let client = std::thread::spawn(move || {
                Client::connect(&s).run("library.trash", json!({ "images": ["a1"] }))
            });
            user_confirms(&mut h, true);
            let trash = finish(&mut h, client);
            assert_eq!(trash["result"]["ok"], true, "{trash}");
            assert_eq!(names(&ran), ["vault.rate", "library.trash"]);

            // Declined, it does not run.
            let s = socket.clone();
            let client = std::thread::spawn(move || {
                Client::connect(&s).run("library.trash", json!({ "images": ["a2"] }))
            });
            user_confirms(&mut h, false);
            let declined = finish(&mut h, client);
            assert!(declined.get("error").is_some(), "{declined}");
            assert_eq!(names(&ran), ["vault.rate", "library.trash"]);
            assert_eq!(refused_by(&h, "pop.agent.confirm").len(), 1);
        }

        #[test]
        fn an_agent_s_call_with_wrong_arguments_is_refused_before_it_runs() {
            let (mut h, socket, ran) = app("schema");
            let s = socket.clone();
            let client = std::thread::spawn(move || {
                Client::connect(&s).run("library.trash", json!({ "images": "a1" }))
            });
            let wrong = finish(&mut h, client);
            // Refused at once, with no confirmation asked.
            assert_eq!(wrong["result"]["ok"], false, "{wrong}");
            assert!(names(&ran).is_empty(), "the handler ran");
            let refused = refused_by(&h, "pop.command.args");
            assert_eq!(refused.len(), 1, "{refused:?}");
            assert_eq!(refused[0]["command"], "library.trash");
        }
    }
}
