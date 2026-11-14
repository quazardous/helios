//! A workshop run's reads and writes as the node's commands
//! (`docs/protocol.md`, "Workshop runs"), so the desktop's run and its
//! review never hold the vault:
//!
//! - `workshop.photo` (`image`) reads what a run opens a photo with: its
//!   head version (null: never edited), the file read (null: none at
//!   hand), that file's name and the photo's rows (a review's revert
//!   takes the run's version out of them).
//! - `workshop.version.add` (`version`) writes a version and answers the
//!   photo's rows before and after it (`change`), for the run's library
//!   step.
//! - `workshop.book.get` / `.set` (`workshop`, id@version; `book`) read
//!   and write a workshop version's decision book.
//! - `workshop.record.get` / `.set` (`run`; `record`, null drops it) read
//!   and write a run's record, which its review reads.
//!
//! A review's revert walks its change forward with `library.changes.apply`.

use helios_vault::{Change, ImageId, Store, Vault, Version, edit};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, Need, object};

/// Reads what a run opens a photo with.
pub const PHOTO: &str = "workshop.photo";

/// Writes a version, answering the photo's rows before and after.
pub const VERSION_ADD: &str = "workshop.version.add";

/// Reads a workshop version's decision book.
pub const BOOK_GET: &str = "workshop.book.get";

/// Writes a workshop version's decision book.
pub const BOOK_SET: &str = "workshop.book.set";

/// Reads a run's record.
pub const RECORD_GET: &str = "workshop.record.get";

/// Writes (or drops) a run's record.
pub const RECORD_SET: &str = "workshop.record.set";

/// The commands, in [`specs`]' order.
pub const IDS: &[&str] = &[
    PHOTO,
    VERSION_ADD,
    BOOK_GET,
    BOOK_SET,
    RECORD_GET,
    RECORD_SET,
];

/// Whether `id` is one of the commands run here.
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// The commands' specs.
pub fn specs() -> Vec<CommandSpec> {
    let spec = |id: &str, label: &str| CommandSpec::new(id, label, "Workshop").needs(Need::Vault);
    let image = || {
        (
            "image",
            json!({ "type": "string", "description": "an image's id (64 hex digits)" }),
            true,
        )
    };
    let workshop = || {
        (
            "workshop",
            json!({ "type": "string", "description": "the workshop's composite, id@version" }),
            true,
        )
    };
    let run = || {
        (
            "run",
            json!({ "type": "string", "description": "the run's id" }),
            true,
        )
    };
    vec![
        spec(PHOTO, "Read what a workshop run opens a photo with")
            .params(object(&[image()]))
            .read_only(),
        spec(VERSION_ADD, "Write a version of a workshop run").params(object(&[(
            "version",
            json!({ "type": "object",
                    "description": "the version (id, image, parents, edit, origin, at, label, meta)" }),
            true,
        )])),
        spec(BOOK_GET, "Read a workshop's decision book")
            .params(object(&[workshop()]))
            .read_only(),
        spec(BOOK_SET, "Write a workshop's decision book").params(object(&[
            workshop(),
            (
                "book",
                json!({ "type": "object", "description": "the decision book" }),
                true,
            ),
        ])),
        spec(RECORD_GET, "Read a workshop run's record")
            .params(object(&[run()]))
            .read_only(),
        spec(RECORD_SET, "Write a workshop run's record").params(object(&[
            run(),
            (
                "record",
                json!({ "type": ["object", "null"], "description": "the run's record; null drops it" }),
                true,
            ),
        ])),
    ]
}

/// Runs command `id` (params already checked against its schema) on
/// `vault`.
///
/// # Errors
/// The vault's refusal, or params that do not read.
pub fn run<S: Store>(vault: &Vault<S>, id: &str, params: &Value) -> Result<Value, String> {
    let text = |key: &str| params[key].as_str().unwrap_or_default();
    match id {
        PHOTO => {
            let image = ImageId::from_hex(text("image"))?;
            let (_, head) = edit::EditSession::open(vault, image)?;
            let file = vault.file_to_read(&image)?.map(|l| l.path);
            let locations = vault.locations(&image)?;
            let order = vault.read_order()?;
            let name = order
                .pick(&locations)
                .or(locations.first())
                .and_then(|l| l.path.file_name())
                .map(|n| n.to_string_lossy().into_owned());
            let rows = vault.rows(&image)?;
            Ok(json!({ "head": head, "file": file, "name": name, "rows": rows }))
        }
        VERSION_ADD => {
            let version: Version = serde_json::from_value(params["version"].clone())
                .map_err(|e| format!("a version: {e}"))?;
            let before = vault.rows(&version.image)?;
            vault.add_version(&version)?;
            let after = vault.rows(&version.image)?;
            let change = Change {
                image: version.image,
                before,
                after,
            };
            Ok(json!({ "change": change }))
        }
        BOOK_GET => Ok(json!({ "book": vault.workshop_book(text("workshop"))? })),
        BOOK_SET => vault
            .set_workshop_book(text("workshop"), &params["book"])
            .map(|()| json!({ "kept": true })),
        RECORD_GET => Ok(json!({ "record": vault.workshop_run(text("run"))? })),
        RECORD_SET => {
            let record = Some(&params["record"]).filter(|r| !r.is_null());
            vault
                .set_workshop_run(text("run"), record)
                .map(|()| json!({ "kept": record.is_some() }))
        }
        other => Err(format!("no command `{other}` here")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::validate;
    use helios_vault::MemoryStore;

    #[test]
    fn every_workshop_command_is_declared_once_with_a_schema_its_example_fits() {
        let ids: Vec<String> = specs().iter().map(|s| s.id.clone()).collect();
        assert_eq!(ids, IDS);
        let examples = [
            json!({ "image": "0a" }),
            json!({ "version": { "id": "0a" } }),
            json!({ "workshop": "backlit@1" }),
            json!({ "workshop": "backlit@1", "book": { "answers": {}, "runs": 2 } }),
            json!({ "run": "workshop-run-1700000000000-1" }),
            json!({ "run": "workshop-run-1700000000000-1", "record": null }),
        ];
        assert_eq!(examples.len(), IDS.len(), "one example per command");
        for (spec, example) in specs().iter().zip(examples) {
            assert_eq!(
                spec.read_only,
                [PHOTO, BOOK_GET, RECORD_GET].contains(&spec.id.as_str())
            );
            validate(&spec.params, &example)
                .unwrap_or_else(|e| panic!("{}: its example does not fit: {e}", spec.id));
        }
    }

    #[test]
    fn a_run_reads_and_writes_its_photo_book_and_record_through_the_node() {
        let vault = Vault::new(MemoryStore::default());
        let dir = std::env::temp_dir().join(format!("helios-workshops-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("IMG_0001.jpg");
        std::fs::write(&path, b"a photo").unwrap();
        let image = vault.import_file(&path, 1).unwrap();
        let hex = image.hex();

        let opened = run(&vault, PHOTO, &json!({ "image": hex })).unwrap();
        assert!(opened["head"].is_null(), "{opened}");
        assert_eq!(opened["name"], "IMG_0001.jpg");
        assert_eq!(opened["file"], json!(path));

        let version = Version::new(image, Vec::new(), json!({ "modules": [] }), "user", 2);
        let added = run(
            &vault,
            VERSION_ADD,
            &json!({ "version": serde_json::to_value(&version).unwrap() }),
        )
        .unwrap();
        let change: Change = serde_json::from_value(added["change"].clone()).unwrap();
        assert_eq!(change.image, image);
        assert_ne!(change.before, change.after);
        let opened = run(&vault, PHOTO, &json!({ "image": hex })).unwrap();
        let head: Version = serde_json::from_value(opened["head"].clone()).unwrap();
        assert_eq!(head.id, version.id);

        let book = json!({ "answers": {}, "runs": 1 });
        run(
            &vault,
            BOOK_SET,
            &json!({ "workshop": "w@1", "book": book }),
        )
        .unwrap();
        assert_eq!(
            run(&vault, BOOK_GET, &json!({ "workshop": "w@1" })).unwrap()["book"],
            book
        );

        let record = json!({ "run": "r-1", "photos": [] });
        run(
            &vault,
            RECORD_SET,
            &json!({ "run": "r-1", "record": record }),
        )
        .unwrap();
        assert_eq!(
            run(&vault, RECORD_GET, &json!({ "run": "r-1" })).unwrap()["record"],
            record
        );
        run(&vault, RECORD_SET, &json!({ "run": "r-1", "record": null })).unwrap();
        assert!(run(&vault, RECORD_GET, &json!({ "run": "r-1" })).unwrap()["record"].is_null());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
