//! The node link's baseline (#5318): today's link between the desktop
//! and a node, and bus-bridge's Unix socket link, measured side by side
//! on the same pair of Helios buses.
//!
//! Each side is a node's fact bus (journaled, with its application bus
//! as journal and relay, as `helios-node` builds it) and a desktop's,
//! one process, two threads of links:
//!
//! - **today**: the node serves its inspect endpoint (`subscribe` with
//!   `relay`) and a control endpoint; the desktop follows it with
//!   [`relay::Relay`], which takes the relayable facts in through its
//!   bus's relay, and asks the control endpoint over a fresh connection
//!   per request, as Cancel does;
//! - **bridge**: a `bus_bridge::Endpoint` on each bus, joined over a Unix
//!   socket; the desktop's request is answered by the node's listeners
//!   (`Endpoint::listen_far`).
//!
//! Measured for each: a fact's latency (published on the node, heard by
//! a listener of its point on the desktop), a burst's throughput, the
//! time to hear a fact published right after the link dropped, and a
//! request's round trip. What is asserted is only what both links
//! promise: every fact heard once, in order, and every request answered.
//! The figures go to stderr and, on the bench box, to
//! `/out/node-link-baseline.json`.

#![cfg(unix)]

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bus_bridge::unix::{UnixAcceptor, UnixDialer};
use bus_bridge::{Config, Endpoint};
use egui_pop::api::bus::bus_contract::{
    self, Facet, ListenOpts, Point, PointDecl, PointId, PointKey, PointKind, declare_all,
};
use egui_pop::api::bus::bus_contract_crossing::{
    Origin, Relayed, RelayedRequest, relayable_request,
};
use egui_pop::api::bus::egui_pop_hooks::Kernel;
use helios_core::jobs::Jobs;
use helios_core::{Bus, BusJournal, Event, FactPoint};
use helios_gui::node_events::NodeEvents;
use helios_gui::relay::{NodeLink, Relay, Timing};
use helios_inspect::control::ControlRequest;
use helios_inspect::{Address, Client, Inspector, Listen};
use helios_node_core::settings::LayerChanged;
use serde_json::{Value, json};

const LONG: Duration = Duration::from_secs(20);
/// Facts measured one at a time, and requests.
const ONE_BY_ONE: u64 = 200;
/// A burst, within the journal's window (1024).
const BURST: u64 = 800;
/// Drops of the link.
const DROPS: u64 = 20;
/// Both links dial again this soon after a drop.
const RETRY: Duration = Duration::from_millis(20);

/// A request answered on the node: its value is its input plus one.
struct Ping;
impl Point for Ping {
    const ID: PointId = PointId::new("bench.ping");
    const KIND: PointKind = PointKind::Request;
    type Input = u64;
    type Value = u64;
}
impl Relayed for Ping {
    fn encode(n: &u64) -> String {
        n.to_string()
    }
    fn decode(text: &str) -> Result<u64, String> {
        text.parse().map_err(|e| format!("{e}"))
    }
}
impl RelayedRequest for Ping {
    fn encode_value(n: &u64) -> String {
        n.to_string()
    }
    fn decode_value(text: &str) -> Result<u64, String> {
        text.parse().map_err(|e| format!("{e}"))
    }
}
const PING_FACETS: &[Facet] = &[relayable_request::<Ping>()];

/// A process's buses: its fact bus, and its application bus with the
/// fact bus as journal and relay.
struct Side {
    facts: Arc<Bus>,
    kernel: Arc<Kernel>,
}

impl Side {
    fn new() -> Side {
        let facts = helios_gui::bus();
        let (kernel, journal) = BusJournal::attach(
            facts.clone(),
            |j| Kernel::new().with_journal(j.clone()).with_relay(j),
            Kernel::declaration,
        );
        let refused = journal.declare(&*kernel, helios_gui::FACTS.iter().copied());
        assert!(refused.is_empty(), "{refused:?}");
        let refused = declare_all(
            &*kernel,
            &[PointDecl::of::<Ping>("A ping, answered on the node").facets(PING_FACETS)],
        );
        assert!(refused.is_empty(), "{refused:?}");
        Side { facts, kernel }
    }

    fn bus(&self) -> Arc<dyn bus_contract::Bus> {
        self.kernel.clone()
    }

    fn publish(&self, n: u64) {
        self.facts.publish_value(
            "settings",
            None,
            LayerChanged::KIND,
            json!({ "layer": "user", "n": n }),
        );
    }

    /// The `n` of every `helios.settings.layerChanged` heard here, as it
    /// is heard.
    fn hear(&self) -> Receiver<u64> {
        let (tx, rx) = channel();
        let tx = Mutex::new(tx);
        self.bus()
            .listen::<FactPoint<LayerChanged>>("bench.heard", ListenOpts::new(), move |f, ()| {
                let _ = tx
                    .lock()
                    .unwrap()
                    .send(f.payload["n"].as_u64().unwrap_or(0));
                Ok(())
            })
            .unwrap();
        rx
    }

    /// Answers [`Ping`] here.
    fn answer_pings(&self) {
        self.bus()
            .listen::<Ping>("bench.pong", ListenOpts::new(), |n, v| {
                *v = n + 1;
                Ok(())
            })
            .unwrap();
    }
}

fn dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("helios-node-link-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A link under measure: a way to drop it and to ask the node.
trait Link {
    fn drop_link(&self);
    fn ask(&self, n: u64) -> u64;
}

struct Today {
    relay: Relay,
    control: Address,
}

impl Link for Today {
    fn drop_link(&self) {
        self.relay.drop_link();
    }
    fn ask(&self, n: u64) -> u64 {
        // As Cancel asks today: a connection per request.
        let mut client = Client::connect(&self.control).unwrap();
        client.set_read_timeout(Some(LONG)).unwrap();
        let answer = client
            .request(&ControlRequest::JobCancel { id: n.to_string() })
            .unwrap();
        answer["n"].as_u64().expect("answered")
    }
}

struct Bridge {
    desktop: Side,
    end: Endpoint,
}

impl Link for Bridge {
    fn drop_link(&self) {
        self.end.disconnect();
    }
    fn ask(&self, n: u64) -> u64 {
        let out = self.desktop.bus().alter::<Ping>(n, 0);
        assert!(out.refused.is_none(), "{:?}", out.refused);
        out.value
    }
}

/// Today's link: the node's inspect and control endpoints, followed by a
/// [`Relay`].
fn today(node: &Side, desktop: &Side, dir: &std::path::Path) -> Today {
    let jobs = Jobs::new(node.facts.clone());
    let inspector = Inspector::new(node.facts.clone())
        .with_jobs()
        .with_relay(node.kernel.clone());
    helios_inspect::serve(inspector, &dir.join("node.sock")).unwrap();
    helios_inspect::serve_at(
        &Listen::Unix(dir.join("control.sock")),
        None,
        move |request: ControlRequest| {
            let _ = &jobs;
            match request {
                ControlRequest::JobCancel { id } => {
                    json!({ "ok": true, "n": id.parse::<u64>().unwrap_or(0) + 1 })
                }
                _ => json!({ "ok": false }),
            }
        },
    )
    .unwrap();
    let shell = Arc::new(Bus::new());
    let link = NodeLink {
        name: "box".into(),
        inspect: Address::Unix(dir.join("node.sock")),
        control: Some(Address::Unix(dir.join("control.sock"))),
    };
    let timing = Timing {
        retry: RETRY,
        forget_after: Duration::from_mins(10),
        silence: Duration::from_secs(5),
    };
    let relay = Relay::start(
        link,
        shell.clone(),
        NodeEvents::new(shell),
        Some(desktop.bus()),
        timing,
    )
    .unwrap();
    Today {
        relay,
        control: Address::Unix(dir.join("control.sock")),
    }
}

/// bus-bridge's link over a Unix socket: the node accepts, the desktop
/// dials.
fn bridge(node: &Side, desktop: Side, dir: &std::path::Path) -> (Bridge, Endpoint) {
    node.answer_pings();
    let path = dir.join("bridge.sock");
    let acceptor = UnixAcceptor::bind(&path).unwrap();
    let node_end =
        Endpoint::new(node.bus(), Config::new("node", Origin::process("desktop"))).unwrap();
    node_end.accept(acceptor);
    let end = Endpoint::new(
        desktop.bus(),
        Config::new("desktop", Origin::process("node")).reconnect(RETRY),
    )
    .unwrap();
    end.connect(UnixDialer::new(&path));
    assert!(end.wait_connected(LONG) && node_end.wait_connected(LONG));
    end.listen_far(
        PointKey::of::<Ping>(),
        "bench.far",
        Duration::from_secs(5),
        ListenOpts::new(),
    )
    .unwrap();
    (Bridge { desktop, end }, node_end)
}

/// Waits for `want` on `heard`, every fact before it in order and once.
fn heard_up_to(heard: &Receiver<u64>, next: &mut u64, want: u64) {
    while *next <= want {
        let n = heard
            .recv_timeout(LONG)
            .unwrap_or_else(|_| panic!("fact {next} not heard within {LONG:?}"));
        assert_eq!(n, *next, "each fact once, in order");
        *next += 1;
    }
}

fn micros(d: &[Duration]) -> Value {
    let mut us: Vec<u64> = d.iter().map(|d| d.as_micros() as u64).collect();
    us.sort_unstable();
    let at = |q: f64| us[((us.len() - 1) as f64 * q).round() as usize];
    json!({ "n": us.len(), "p50_us": at(0.5), "p95_us": at(0.95), "max_us": at(1.0) })
}

/// Measures `link` between `node` and the desktop's `heard`.
fn measure(node: &Side, heard: &Receiver<u64>, link: &dyn Link) -> Value {
    // Up: the first fact said until it is heard (today's link starts from
    // the node's present).
    let start = Instant::now();
    let deadline = start + LONG;
    let mut n = 0;
    loop {
        node.publish(n);
        if let Ok(got) = heard.recv_timeout(Duration::from_millis(20)) {
            n = got + 1;
            break;
        }
        n += 1;
        assert!(Instant::now() < deadline, "the link never came up");
    }
    // Anything still in flight from the warm-up.
    std::thread::sleep(Duration::from_millis(200));
    while let Ok(got) = heard.try_recv() {
        n = got + 1;
    }
    let up = start.elapsed();
    let mut next = n;

    let mut latency = Vec::new();
    for _ in 0..ONE_BY_ONE {
        let t = Instant::now();
        node.publish(next);
        let want = next;
        heard_up_to(heard, &mut next, want);
        latency.push(t.elapsed());
    }

    let t = Instant::now();
    let last = next + BURST - 1;
    for k in next..=last {
        node.publish(k);
    }
    let published = t.elapsed();
    heard_up_to(heard, &mut next, last);
    let burst = t.elapsed();

    let mut resume = Vec::new();
    for _ in 0..DROPS {
        link.drop_link();
        let t = Instant::now();
        node.publish(next);
        let want = next;
        heard_up_to(heard, &mut next, want);
        resume.push(t.elapsed());
    }

    let mut asks = Vec::new();
    for k in 0..ONE_BY_ONE {
        let t = Instant::now();
        assert_eq!(link.ask(k), k + 1, "the node's answer");
        asks.push(t.elapsed());
    }

    std::thread::sleep(Duration::from_millis(100));
    assert!(heard.try_recv().is_err(), "nothing heard twice");
    json!({
        "up_ms": up.as_millis() as u64,
        "fact_latency": micros(&latency),
        "burst": {
            "facts": BURST,
            "publish_ms": published.as_millis() as u64,
            "heard_ms": burst.as_millis() as u64,
            "facts_per_s": (BURST as f64 / burst.as_secs_f64()).round() as u64,
        },
        "after_drop": micros(&resume),
        "request": micros(&asks),
    })
}

fn report(name: &str, figures: &Value) {
    eprintln!("node link baseline, {name}: {figures:#}");
    let out = std::path::Path::new("/out");
    if out.is_dir() {
        let file = out.join("node-link-baseline.json");
        let mut all: Value = std::fs::read_to_string(&file)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_else(|| json!({}));
        all[name] = figures.clone();
        let _ = std::fs::write(&file, format!("{all:#}\n"));
    }
}

#[test]
fn today_s_node_link_delivers_each_fact_once_and_answers() {
    let dir = dir("today");
    let (node, desktop) = (Side::new(), Side::new());
    let heard = desktop.hear();
    let link = today(&node, &desktop, &dir);
    let figures = measure(&node, &heard, &link);
    report("today", &figures);
    drop(link);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn bus_bridge_over_a_unix_socket_delivers_each_fact_once_and_answers() {
    let dir = dir("bridge");
    let (node, desktop) = (Side::new(), Side::new());
    let heard = desktop.hear();
    let (link, node_end) = bridge(&node, desktop, &dir);
    let mut figures = measure(&node, &heard, &link);
    let stats = link.end.stats();
    figures["desktop_stats"] = json!({
        "connections": stats.connections,
        "lost": stats.lost,
    });
    report("bridge", &figures);
    drop(link);
    drop(node_end);
    let _ = std::fs::remove_dir_all(&dir);
}
