//! `cargo xtask lint [--fix] [--require-all]`: every linter and formatter
//! of the project in one command.
//!
//! rustfmt and clippy always run. The other tools run when installed and
//! are reported as skipped otherwise: cargo-deny (licenses, advisories,
//! sources), cargo-machete (unused dependencies), taplo (TOML formatting),
//! typos (spelling), shellcheck (shell scripts) and ruff (Python). CI
//! installs them all and passes `--require-all`, which turns a missing tool
//! into a failure.
//!
//! `--fix` applies what can be applied (rustfmt, taplo, typos, ruff) before
//! checking.

use std::path::Path;
use std::process::{Command, ExitCode, Stdio};

pub const USAGE: &str = "lint [--fix] [--require-all]";

/// One external program, and how to say it is missing.
struct Tool {
    name: &'static str,
    /// The command that answers when the tool is installed.
    probe: &'static [&'static str],
    install: &'static str,
}

const CARGO_DENY: Tool = Tool {
    name: "cargo-deny",
    probe: &["cargo", "deny", "--version"],
    install: "cargo install --locked cargo-deny",
};
const CARGO_MACHETE: Tool = Tool {
    name: "cargo-machete",
    probe: &["cargo", "machete", "--version"],
    install: "cargo install --locked cargo-machete",
};
const TAPLO: Tool = Tool {
    name: "taplo",
    probe: &["taplo", "--version"],
    install: "cargo install --locked taplo-cli",
};
const TYPOS: Tool = Tool {
    name: "typos",
    probe: &["typos", "--version"],
    install: "cargo install --locked typos-cli",
};
const SHELLCHECK: Tool = Tool {
    name: "shellcheck",
    probe: &["shellcheck", "--version"],
    install: "your distribution's shellcheck package",
};
const RUFF: Tool = Tool {
    name: "ruff",
    probe: &["ruff", "--version"],
    install: "pip install ruff",
};

impl Tool {
    fn installed(&self) -> bool {
        Command::new(self.probe[0])
            .args(&self.probe[1..])
            .env_remove("CARGO_PKG_NAME")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }
}

/// Runs the steps in order, keeps going after a failure, and reports.
struct Lint<'a> {
    root: &'a Path,
    failed: Vec<String>,
    missing: Vec<&'static Tool>,
}

impl Lint<'_> {
    /// Runs `args` from the workspace root; a failure is remembered.
    fn run(&mut self, step: &str, args: &[&str]) {
        println!("\n==> {step}: {}", args.join(" "));
        let ok = Command::new(args[0])
            .args(&args[1..])
            // Set by `cargo run` for xtask itself; cargo-machete takes it as
            // a sign it runs as a cargo subcommand and misreads its arguments.
            .env_remove("CARGO_PKG_NAME")
            .current_dir(self.root)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            self.failed.push(step.to_string());
        }
    }

    /// Whether `tool` can run; a missing one is remembered and skipped.
    fn has(&mut self, tool: &'static Tool) -> bool {
        if tool.installed() {
            return true;
        }
        println!("\n==> {}: not installed, skipped", tool.name);
        if !self.missing.iter().any(|t| t.name == tool.name) {
            self.missing.push(tool);
        }
        false
    }

    /// The files git tracks that match `patterns`.
    fn tracked(&self, patterns: &[&str]) -> Vec<String> {
        let out = Command::new("git")
            .arg("ls-files")
            .arg("--")
            .args(patterns)
            .current_dir(self.root)
            .output();
        match out {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(str::to_string)
                .collect(),
            _ => Vec::new(),
        }
    }
}

pub fn run(root: &Path, args: &[&str]) -> Option<ExitCode> {
    let (mut fix, mut require_all) = (false, false);
    for a in args {
        match *a {
            "--fix" => fix = true,
            "--require-all" => require_all = true,
            _ => return None,
        }
    }
    let mut lint = Lint {
        root,
        failed: Vec::new(),
        missing: Vec::new(),
    };

    if fix {
        lint.run("rustfmt", &["cargo", "fmt", "--all"]);
    } else {
        lint.run("rustfmt", &["cargo", "fmt", "--all", "--check"]);
    }
    lint.run(
        "clippy",
        &[
            "cargo",
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
    );
    if lint.has(&CARGO_DENY) {
        lint.run("cargo-deny", &["cargo", "deny", "check"]);
    }
    if lint.has(&CARGO_MACHETE) {
        // The workspace's crates only: the GUI prototypes in proto/ build
        // on their own.
        lint.run(
            "cargo-machete",
            &["cargo", "machete", "crates", "modules", "plugins", "xtask"],
        );
    }
    if lint.has(&TAPLO) {
        if fix {
            lint.run("taplo", &["taplo", "fmt"]);
        } else {
            lint.run("taplo", &["taplo", "fmt", "--check"]);
        }
    }
    if lint.has(&TYPOS) {
        if fix {
            lint.run("typos", &["typos", "--write-changes"]);
        }
        lint.run("typos", &["typos"]);
    }
    let scripts = lint.tracked(&["*.sh", "tools/pre-commit"]);
    if !scripts.is_empty() && lint.has(&SHELLCHECK) {
        let mut args = vec!["shellcheck"];
        args.extend(scripts.iter().map(String::as_str));
        lint.run("shellcheck", &args);
    }
    let python = lint.tracked(&["*.py"]);
    if !python.is_empty() && lint.has(&RUFF) {
        if fix {
            lint.run("ruff", &["ruff", "check", "--fix"]);
            lint.run("ruff format", &["ruff", "format"]);
        } else {
            lint.run("ruff", &["ruff", "check"]);
            lint.run("ruff format", &["ruff", "format", "--check"]);
        }
    }

    println!();
    for tool in &lint.missing {
        println!("missing: {} (install: {})", tool.name, tool.install);
    }
    if require_all && !lint.missing.is_empty() {
        lint.failed.push("required tools are missing".into());
    }
    Some(if lint.failed.is_empty() {
        println!("Lint OK.");
        ExitCode::SUCCESS
    } else {
        lint.failed.dedup();
        eprintln!("xtask: lint failed: {}", lint.failed.join(", "));
        ExitCode::from(1)
    })
}
