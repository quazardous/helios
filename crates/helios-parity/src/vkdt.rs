//! Parity with vkdt: a module ported from vkdt is checked against vkdt's
//! own render, through `vkdt-cli`.
//!
//! The same float image goes to both. vkdt reads it with `i-pfm` (which, as
//! `o-pfm`, does not flip rows: rows top first), runs the module, and
//! writes through the `display` node that `vkdt-cli --format o-pfm`
//! replaces, asked for linear Rec.2020 so that no colour encoding is
//! inserted. The pipe stays in 32-bit floats from `i-pfm` on.
//!
//! vkdt-cli is found as `VKDT_CLI`, else `vkdt-cli` on the PATH; it finds
//! its modules next to itself (a vkdt checkout's `bin/`).

use std::path::{Path, PathBuf};
use std::process::Command;

use helios_core::{Image, Region};

pub struct Vkdt {
    pub cli: PathBuf,
}

/// Writes RGB as a PFM with rows top first, as vkdt's `i-pfm` reads it.
pub fn write_pfm_top_first(image: &Image, path: &Path) -> Result<(), String> {
    let r = image.region();
    let mut out = format!("PF\n{} {}\n-1.0\n", r.width, r.height).into_bytes();
    for y in 0..r.height as i64 {
        for x in 0..r.width as i64 {
            let p = image.pixel(r.x + x, r.y + y);
            for c in 0..3 {
                out.extend_from_slice(&p[c.min(p.len() - 1)].to_le_bytes());
            }
        }
    }
    std::fs::write(path, out).map_err(|e| format!("{}: {e}", path.display()))
}

/// Reads a PFM written by vkdt's `o-pfm` (rows top first).
pub fn read_pfm_top_first(path: &Path) -> Result<Image, String> {
    // The standard reader flips rows; flip them back.
    let flipped = crate::read_pfm(path)?;
    let r = flipped.region();
    Ok(Image::from_fn(
        Region::sized(r.width, r.height),
        3,
        |x, y, c| flipped.pixel(x, r.height as i64 - 1 - y)[c as usize],
    ))
}

impl Vkdt {
    /// vkdt-cli, or why there is none.
    pub fn find() -> Result<Vkdt, String> {
        let cli = std::env::var_os("VKDT_CLI")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("vkdt-cli"));
        // vkdt-cli without arguments prints its usage and fails: it exists.
        Command::new(&cli)
            .output()
            .map_err(|e| format!("no vkdt-cli ({}): {e}", cli.display()))?;
        Ok(Vkdt { cli })
    }

    /// Renders `input` through one vkdt module with `params` (name, value
    /// as vkdt's cfg writes it), in a scratch directory `dir`.
    pub fn render(
        &self,
        input: &Image,
        module: &str,
        params: &[(&str, String)],
        dir: &Path,
    ) -> Result<Image, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let src = dir.join("input.pfm");
        write_pfm_top_first(input, &src)?;
        let out = dir.join("output");
        let mut cfg = format!(
            "module:i-pfm:main\nmodule:{module}:01\nmodule:display:main\n\
             connect:i-pfm:main:output:{module}:01:input\n\
             connect:{module}:01:output:display:main:input\n\
             param:i-pfm:main:filename:{}\n",
            src.display()
        );
        for (name, value) in params {
            cfg.push_str(&format!("param:{module}:01:{name}:{value}\n"));
        }
        let graph = dir.join("graph.cfg");
        std::fs::write(&graph, cfg).map_err(|e| format!("{}: {e}", graph.display()))?;
        let run = Command::new(&self.cli)
            .arg("-g")
            .arg(&graph)
            .args(["--format", "o-pfm", "--filename"])
            .arg(&out)
            .args(["--colour-prim", "bt2020", "--colour-trc", "linear"])
            .output()
            .map_err(|e| format!("vkdt-cli: {e}"))?;
        let written = out.with_extension("pfm");
        if !run.status.success() || !written.exists() {
            return Err(format!(
                "vkdt-cli failed ({}): {}{}",
                run.status,
                String::from_utf8_lossy(&run.stdout),
                String::from_utf8_lossy(&run.stderr)
            ));
        }
        read_pfm_top_first(&written)
    }
}
