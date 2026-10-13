//! `helios-corpus`: runs the raw corpus checks on a directory and reports.
//!
//! ```text
//! helios-corpus [--dir DIR] [--out DIR] [--bless]
//! ```
//!
//! - `--dir`: the corpus directory (filled by `tools/bench/fill-corpus.sh`),
//!   `$HELIOS_RAW_CORPUS` by default;
//! - `--out`: where to write `junit.xml`, `summary.md`, `observed.toml`
//!   (what each file gave, as manifest tables) and `thumbs/<id>.ppm`;
//! - `--bless`: accept the thumbnails as they are now: rewrites
//!   `tests/raw-corpus.thumbs.toml` (and copies it to `--out`) and the
//!   blessed images of `tests/raw-corpus-thumbs/`; the other checks still
//!   fail the run.
//!
//! Exits 0 when every file passes, 1 when one fails, 2 on a usage or I/O
//! error.

use std::path::PathBuf;
use std::process::ExitCode;

use helios_corpus::{
    Manifest, THUMB_IMAGES, junit, observed_toml, read_thumbs, repo_root, run, summary,
    write_thumbs,
};

const USAGE: &str = "usage: helios-corpus [--dir DIR] [--out DIR] [--bless]";

fn main() -> ExitCode {
    match main_() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("helios-corpus: {e}");
            ExitCode::from(2)
        }
    }
}

fn main_() -> Result<ExitCode, String> {
    let mut dir = std::env::var_os("HELIOS_RAW_CORPUS").map(PathBuf::from);
    let mut out = None;
    let mut bless = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--dir" => dir = Some(args.next().ok_or(USAGE)?.into()),
            "--out" => out = Some(PathBuf::from(args.next().ok_or(USAGE)?)),
            "--bless" => bless = true,
            _ => return Err(format!("unknown argument {a:?}\n{USAGE}")),
        }
    }
    let dir =
        dir.ok_or_else(|| format!("no corpus directory: --dir or HELIOS_RAW_CORPUS\n{USAGE}"))?;
    let root = repo_root();
    let manifest = Manifest::read(&root.join("tests/raw-corpus.toml"))?;
    let thumbs_path = root.join("tests/raw-corpus.thumbs.toml");
    let mut thumbs = read_thumbs(&thumbs_path)?;
    let mut outcomes = run(&dir, &manifest, &thumbs);

    if bless {
        for o in &mut outcomes {
            if let Some(t) = &o.observed.thumb {
                thumbs.insert(o.id.clone(), t.clone());
            }
            o.failures.retain(|f| !f.starts_with("thumbnail:"));
            o.notes.retain(|n| !n.starts_with("thumbnail"));
        }
        thumbs.retain(|id, _| manifest.checked().any(|e| &e.id == id));
        write_thumbs(&thumbs_path, &thumbs)?;
        let images = root.join(THUMB_IMAGES);
        std::fs::create_dir_all(&images).map_err(|e| format!("{}: {e}", images.display()))?;
        for o in &outcomes {
            if let Some(t) = &o.thumb {
                let path = images.join(format!("{}.ppm", o.id));
                std::fs::write(&path, t.ppm()).map_err(|e| format!("{}: {e}", path.display()))?;
            }
        }
        // the images of files no longer checked go
        for entry in std::fs::read_dir(&images)
            .map_err(|e| e.to_string())?
            .flatten()
        {
            let name = entry.file_name().to_string_lossy().to_string();
            let id = name.strip_suffix(".ppm").unwrap_or(&name);
            if !thumbs.contains_key(id) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        println!(
            "blessed {} thumbnails in {}",
            thumbs.len(),
            thumbs_path.display()
        );
    }
    let failed: Vec<_> = outcomes.iter().filter(|o| !o.failures.is_empty()).collect();

    if let Some(out) = &out {
        let io = |p: PathBuf, data: &[u8]| {
            std::fs::write(&p, data).map_err(|e| format!("{}: {e}", p.display()))
        };
        std::fs::create_dir_all(out.join("thumbs")).map_err(|e| e.to_string())?;
        io(out.join("junit.xml"), junit(&outcomes).as_bytes())?;
        io(out.join("summary.md"), summary(&outcomes).as_bytes())?;
        io(
            out.join("observed.toml"),
            observed_toml(&outcomes).as_bytes(),
        )?;
        for o in &outcomes {
            if let Some(t) = &o.thumb {
                io(out.join("thumbs").join(format!("{}.ppm", o.id)), &t.ppm())?;
            }
        }
        if bless {
            std::fs::copy(&thumbs_path, out.join("raw-corpus.thumbs.toml"))
                .map_err(|e| e.to_string())?;
        }
    }

    for o in &outcomes {
        for n in &o.notes {
            println!("NOTE {}: {n}", o.id);
        }
    }
    for o in &failed {
        for f in &o.failures {
            println!("FAIL {}: {f}", o.id);
        }
    }
    println!(
        "raw corpus: {} files, {} passed, {} failed ({})",
        outcomes.len(),
        outcomes.len() - failed.len(),
        failed.len(),
        dir.display()
    );
    Ok(if failed.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}
