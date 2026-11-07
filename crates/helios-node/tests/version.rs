//! `helios --version` prints the workspace version: the release workflow's
//! first check of a packaged build.

use std::process::Command;

#[test]
fn the_cli_says_its_version() {
    for flag in ["--version", "-V"] {
        let out = Command::new(env!("CARGO_BIN_EXE_helios"))
            .arg(flag)
            .output()
            .unwrap();
        assert!(out.status.success());
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            format!("helios {}", env!("CARGO_PKG_VERSION"))
        );
    }
}
