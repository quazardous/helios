//! Helios's scenarios run live: egui-pop's scenario command line
//! (`egui_pop_test::scenario::live::cli`), built in this workspace so the
//! box's `scenarios` job (tools/bench/scenarios.py) needs no other
//! checkout. It drives a `helios-gui` already running with its agent
//! socket on (`EGUI_POP_AGENT_SOCKET`) and writable (`HELIOS_AGENT_WRITE=1`);
//! see docs/gui.md, "Agents".
//!
//! ```text
//! scenarios --socket PATH [--out DIR] [--var NAME=VALUE]... [--shot CAPABILITY ARGS_JSON] FILE_OR_DIR...
//! ```

#[cfg(unix)]
fn main() {
    std::process::exit(egui_pop_test::scenario::live::cli::main(
        std::env::args().skip(1).collect(),
    ));
}

#[cfg(not(unix))]
fn main() {
    eprintln!("live scenarios need a Unix socket");
    std::process::exit(2);
}
