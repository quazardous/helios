//! The node's relayable facts served over bus-bridge (Unix only).
//!
//! The desktop that follows a node with `--node` takes its relayable
//! facts in through a bus-bridge link (`helios_gui::relay`): the node
//! accepts on a Unix socket next to its inspect socket
//! ([`helios_inspect::bridge_socket`]), one link per desktop, each its
//! own `Endpoint` so that several desktops follow it at once. A link
//! carries the node's facts to the desktop only: the desktop's end is
//! receive-only. The peer check is bus-bridge's default, the same user.

use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use bus_bridge::bus_contract::Bus;
use bus_bridge::bus_contract_crossing::{BusCrossing as _, Origin};
use bus_bridge::transport::{Accept, Stream};
use bus_bridge::unix::UnixAcceptor;
use bus_bridge::{Config, Endpoint};

/// How long a new connection has to finish its handshake.
const HANDSHAKE: Duration = Duration::from_secs(10);

/// Serves the relayable facts of `bus` (the node's application bus,
/// with its fact bus as journal and relay) at `path`, for the life of
/// the process. Returns once the socket is bound.
pub fn serve(bus: Arc<dyn Bus>, path: &Path) -> io::Result<()> {
    if bus.relay().is_none() {
        return Err(io::Error::other("the node's bus has no relay"));
    }
    let acceptor = UnixAcceptor::bind(path)?;
    std::thread::Builder::new()
        .name("helios-node-bridge".into())
        .spawn(move || {
            let mut links: Vec<(Instant, Endpoint)> = Vec::new();
            loop {
                match acceptor.accept(Duration::from_secs(1)) {
                    Ok(Some(stream)) => match link(&bus, stream) {
                        Ok(end) => links.push((Instant::now(), end)),
                        Err(e) => log::warn!(target: "helios::node", "bridge: {e}"),
                    },
                    Ok(None) => {}
                    Err(e) => {
                        log::warn!(target: "helios::node", "bridge: {e}");
                        return;
                    }
                }
                // A link whose desktop went is dropped (its desktop
                // dials again on a new one), as is one that never
                // finished its handshake (a peer refused, a dead dial).
                links.retain(|(at, end)| {
                    let stats = end.stats();
                    stats.connected || (stats.connections == 0 && at.elapsed() < HANDSHAKE)
                });
            }
        })?;
    Ok(())
}

/// One desktop's link, on the connection it opened.
fn link(bus: &Arc<dyn Bus>, stream: Box<dyn Stream>) -> Result<Endpoint, String> {
    let end = Endpoint::new(bus.clone(), Config::new("node", Origin::process("desktop")))
        .map_err(|e| e.to_string())?;
    end.accept(Once(Mutex::new(Some(stream))));
    Ok(end)
}

/// An `Accept` that hands over one connection, then ends.
struct Once(Mutex<Option<Box<dyn Stream>>>);

impl Accept for Once {
    fn accept(&self, _wait: Duration) -> io::Result<Option<Box<dyn Stream>>> {
        match self.0.lock().unwrap_or_else(PoisonError::into_inner).take() {
            Some(stream) => Ok(Some(stream)),
            None => Err(io::Error::other("one connection per link")),
        }
    }
}
