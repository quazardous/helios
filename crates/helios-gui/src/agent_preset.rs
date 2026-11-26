//! What agents on the socket may do when a run asks for more than the
//! defaults: the scenario preset, for scenario and test runs on a
//! throwaway library (docs/gui.md, "Agents").
//!
//! Without a preset, egui-pop-agent's defaults hold: the bridge reads
//! only, an unpaired client reads, a paired agent (approved by the user)
//! edits, destructive calls asked first.
//!
//! `HELIOS_AGENT_PRESET=scenario` makes the bridge writable and issues a
//! pairing at start for the scenario runner, with the scenario grant:
//! every command, value writes and `ui` actions, private data withheld.
//! The token comes from `EGUI_POP_AGENT_TOKEN`, else is written to the
//! file `EGUI_POP_AGENT_TOKEN_FILE` names; the runner says `hello` with
//! it (`examples/scenarios.rs`, `egui_pop_test::scenario::live`). Agents
//! the user pairs get the same grant; unpaired clients still only read.
//! `scenario+private` adds private data (paths, accounts, people,
//! places), which importing a folder or opening a file by its path needs.
//! Destructive calls are still held for the user's yes.
//!
//! `HELIOS_AGENT_WRITE=1`, the switch before the preset, is no longer
//! read: a warning says what replaces it.

use egui_pop_agent::{AgentGrants, Grant, PreIssue, TOKEN_ENV, TOKEN_FILE_ENV};

/// The environment variable naming the preset.
pub const PRESET: &str = "HELIOS_AGENT_PRESET";
/// Removed: the switch before [`PRESET`], only warned about now.
pub const AGENT_WRITE: &str = "HELIOS_AGENT_WRITE";
/// The agent id the scenario runner is paired as.
pub const RUNNER_ID: &str = "scenario";

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
    /// log when it is asked wrongly or by the removed switch.
    pub fn from_env(env: impl Fn(&str) -> Option<String>) -> (Option<Self>, Option<String>) {
        if let Some(name) = env(PRESET).filter(|n| !n.trim().is_empty()) {
            return match Self::parse(&name) {
                Ok(p) => (Some(p), None),
                Err(e) => (None, Some(e)),
            };
        }
        if env(AGENT_WRITE).is_some() {
            return (
                None,
                Some(format!(
                    "{AGENT_WRITE} was removed and is ignored: use {PRESET}=scenario+private \
                     (or scenario) with {TOKEN_ENV} or {TOKEN_FILE_ENV} for the runner"
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

    /// The grants: paired agents get the preset's; unpaired clients keep
    /// egui-pop's read-only default.
    pub fn grants(self) -> AgentGrants {
        AgentGrants::new().paired(self.grant())
    }

    /// The scenario runner's pairing, issued at start with the preset's
    /// grant: its token from [`TOKEN_ENV`], else a new one written to
    /// the file [`TOKEN_FILE_ENV`] names. `Err` with a warning to log
    /// when neither is set: no runner can then write.
    pub fn pre_issue(self, env: impl Fn(&str) -> Option<String>) -> Result<PreIssue, String> {
        let set = |v: &str| env(v).filter(|s| !s.is_empty());
        let issue = if let Some(token) = set(TOKEN_ENV) {
            PreIssue::token(RUNNER_ID, token)
        } else if let Some(file) = set(TOKEN_FILE_ENV) {
            PreIssue::file(RUNNER_ID, file)
        } else {
            return Err(format!(
                "{PRESET} is set but neither {TOKEN_ENV} nor {TOKEN_FILE_ENV}: \
                 no scenario runner is paired, unpaired clients only read"
            ));
        };
        Ok(issue.grant(self.grant()))
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

    /// A token as long as egui-pop asks (32 characters at least).
    const TOKEN: &str = "0123456789abcdef0123456789abcdef-test";

    #[test]
    fn the_preset_is_read_from_its_variable_and_the_old_switch_only_warns() {
        assert_eq!(
            Preset::from_env(env(&[(PRESET, "scenario")])),
            (Some(Preset::Scenario { private: false }), None)
        );
        assert_eq!(
            Preset::from_env(env(&[(PRESET, "scenario+private")])).0,
            Some(Preset::Scenario { private: true })
        );
        // The removed switch grants nothing, and says what replaces it.
        let (p, warning) = Preset::from_env(env(&[(AGENT_WRITE, "1")]));
        assert_eq!(p, None);
        assert!(warning.unwrap().contains("removed"));
        assert_eq!(Preset::from_env(env(&[])), (None, None));
        let (p, error) = Preset::from_env(env(&[(PRESET, "admin")]));
        assert_eq!(p, None);
        assert!(error.unwrap().contains("unknown preset"));
        assert_eq!(
            Preset::from_env(env(&[(PRESET, "scenario"), (AGENT_WRITE, "1")])).0,
            Some(Preset::Scenario { private: false })
        );
    }

    #[test]
    fn the_scenario_grant_goes_to_paired_agents_and_unpaired_clients_only_read() {
        let g = Preset::Scenario { private: false }.grants();
        assert!(g.unpaired.read_only, "unpaired clients keep the default");
        assert_eq!(g.unpaired, AgentGrants::new().unpaired);
        assert!(!g.paired.read_only);
        assert!(g.paired.ui);
        assert!(!g.paired.private);
        assert!(Preset::Scenario { private: true }.grant().private);
    }

    #[test]
    fn the_runner_s_pairing_takes_its_token_from_the_variable_else_the_file() {
        let p = Preset::Scenario { private: true };
        assert_eq!(
            p.pre_issue(env(&[(TOKEN_ENV, TOKEN), (TOKEN_FILE_ENV, "/t")])),
            Ok(PreIssue::token(RUNNER_ID, TOKEN).grant(p.grant()))
        );
        assert_eq!(
            p.pre_issue(env(&[(TOKEN_ENV, ""), (TOKEN_FILE_ENV, "/run/t")])),
            Ok(PreIssue::file(RUNNER_ID, "/run/t").grant(p.grant()))
        );
        assert!(p.pre_issue(env(&[])).unwrap_err().contains(TOKEN_ENV));
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
        use egui_pop_agent::{
            AgentGrantsExt as _, AgentSocket, AgentSocketExt as _, PreIssueExt as _,
        };
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

        /// An application with Helios's host and four of its commands:
        /// `library.trash` (destructive, with a schema), `vault.rate` (a
        /// write), `library.folder` (private) and `library.export`
        /// (destructive with `clash: overwrite`); its agent socket under
        /// the scenario preset, set in code, no variable, the runner's
        /// token [`TOKEN`] issued at start.
        fn app(name: &str) -> (Harness<'static, App>, PathBuf, Ran) {
            app_with(name, Preset::Scenario { private: false })
        }

        /// [`app`] under `preset`.
        fn app_with(name: &str, preset: Preset) -> (Harness<'static, App>, PathBuf, Ran) {
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
                HeliosSpec::new("library.export", "Export…", "Library"),
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
            let builder = App::builder()
                .host(host)
                .plugins(DefaultPlugins)
                .plugin(DeclaresHelios(specs.to_vec()))
                // The Queue panel's verbs and its read (`helios.workflow.*`).
                .plugin(crate::workflow_queue::Commands {
                    relays: crate::jobs::Relays::default(),
                    panel: std::rc::Rc::default(),
                })
                .debug_bridge(egui_pop::core::debug::BridgeMode::On)
                .agent_grants(preset.grants())
                .app_id("org.example.HeliosAgentPreset")
                .agent_socket(AgentSocket::at(&socket))
                .pre_issued_pairing(preset.pre_issue(env(&[(TOKEN_ENV, TOKEN)])).unwrap());
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
            /// A client that says no `hello`.
            fn unpaired(path: &Path) -> Self {
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

            /// The scenario runner: says `hello` with the pre-issued token.
            fn connect(path: &Path) -> Self {
                let mut c = Self::unpaired(path);
                let hello = c.request("hello", json!({ "name": "runner", "token": TOKEN }));
                assert_eq!(hello["result"]["agent"], RUNNER_ID, "{hello}");
                c
            }

            fn request(&mut self, method: &str, params: Json) -> Json {
                let n = self.next;
                self.next += 1;
                let req = json!({ "jsonrpc": "2.0", "id": n, "method": method, "params": params });
                writeln!(self.write, "{req}").unwrap();
                let mut line = String::new();
                self.read.read_line(&mut line).expect("a response line");
                serde_json::from_str(&line).expect("JSON")
            }

            /// `commands.run` of `id`: the call's whole response.
            fn run(&mut self, id: &str, args: Json) -> Json {
                let params = json!({
                    "collector": "commands",
                    "action": "run",
                    "args": { "id": id, "args": args },
                    "deadline_ms": 20_000,
                });
                self.request("call", params)
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
                            values: serde_json::Map::default(),
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
        fn an_unpaired_client_only_reads_under_the_preset() {
            let (mut h, socket, ran) = app("unpaired");
            let s = socket.clone();
            let client = std::thread::spawn(move || {
                Client::unpaired(&s).run("vault.rate", json!({ "stars": 3 }))
            });
            let rate = finish(&mut h, client);
            assert!(
                rate.get("error").is_some() || rate["result"]["ok"] == false,
                "{rate}"
            );
            assert!(
                names(&ran).is_empty(),
                "the write ran for an unpaired client"
            );
        }

        #[test]
        fn only_an_overwriting_export_waits_for_the_user() {
            // An export names file paths: private data.
            let (mut h, socket, ran) = app_with("export", Preset::Scenario { private: true });
            // A new file's name numbered: runs at once.
            let s = socket.clone();
            let client = std::thread::spawn(move || {
                Client::connect(&s).run("library.export", json!({ "clash": "number" }))
            });
            let numbered = finish(&mut h, client);
            assert_eq!(numbered["result"]["ok"], true, "{numbered}");
            assert_eq!(names(&ran), ["library.export"]);

            // Overwriting: held until the user says yes.
            let s = socket.clone();
            let client = std::thread::spawn(move || {
                Client::connect(&s).run("library.export", json!({ "clash": "overwrite" }))
            });
            user_confirms(&mut h, true);
            let overwrite = finish(&mut h, client);
            assert_eq!(overwrite["result"]["ok"], true, "{overwrite}");
            assert_eq!(names(&ran), ["library.export", "library.export"]);
            assert_eq!(ran.lock().unwrap()[1].1["clash"], "overwrite");
        }

        /// The queue's read is for every agent; its verbs for those who
        /// may write: an unpaired client (read only) reads the queue and is
        /// refused a cancel with the reason, the paired runner's cancel
        /// passes the grants and is queued (to fail at the node: none is
        /// connected).
        #[test]
        fn the_queue_is_read_by_any_agent_and_acted_on_by_a_writer() {
            use crate::workflow_queue::{CANCEL, LIST};
            let (mut h, socket, _) = app("queue");
            let s = socket.clone();
            let client = std::thread::spawn(move || {
                let mut c = Client::unpaired(&s);
                (
                    c.run(LIST, json!({ "limit": 10 })),
                    c.run(CANCEL, json!({ "node": "n1", "run": "r1" })),
                )
            });
            let (list, cancel) = finish(&mut h, client);
            assert_eq!(list["result"]["ok"], true, "{list}");
            assert!(cancel.get("error").is_some(), "{cancel}");
            assert!(
                cancel.to_string().contains("pop.agent.grants"),
                "the reason names the grants: {cancel}"
            );

            // A wrong page is refused before the read.
            let s = socket.clone();
            let client =
                std::thread::spawn(move || Client::unpaired(&s).run(LIST, json!({ "limit": 0 })));
            let wrong = finish(&mut h, client);
            assert_eq!(wrong["result"]["ok"], false, "{wrong}");
            assert!(
                refused_by(&h, "pop.command.args")
                    .iter()
                    .any(|r| r["command"] == LIST)
            );

            // The runner may write: the grants let the cancel through.
            let s = socket.clone();
            let client = std::thread::spawn(move || {
                Client::connect(&s).run(CANCEL, json!({ "node": "n1", "run": "r1" }))
            });
            let cancel = finish(&mut h, client);
            assert!(!cancel.to_string().contains("pop.agent.grants"), "{cancel}");
            // Queued: it fails later at the node (none is connected).
            assert_eq!(cancel["result"]["ok"], true, "{cancel}");
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
