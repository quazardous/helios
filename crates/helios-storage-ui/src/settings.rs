//! The Settings app's Storage and Backup pages, declared for its manifest
//! (the desktop places them in its own): the volume table, the backup
//! strategy and the backup wizard, each read and written through the
//! commands of [`crate::admin`].

use egui_pop::api::admin::model::Field;
use egui_pop::api::admin::model::{
    Action, Clauses as _, Confirmation, FieldKind, Records, RowField, SectionBuilder, Status,
    Wizard,
};
use helios_core::Message;
use serde_json::{Value as Json, json};

/// The backup wizard's id: "Set up backups…" on the Backup scope's
/// strategy opens it.
pub const BACKUP_WIZARD: &str = "backup.setup";

/// The backup wizard: how many copies, on which disks, one off site
/// (asked only for two copies or more), when they are made; a summary,
/// then `library.setUpBackups` with the choices.
pub fn backup_wizard(t: &dyn Fn(Message) -> String) -> Wizard {
    use crate::admin::SET_UP_STRATEGY as SET_UP_STRATEGY_COMMAND;
    let field = |key: &str, title: Message, kind| Field::draft(BACKUP_WIZARD, key, &t(title), kind);
    let copies = (1..=5).fold(
        field(
            "copies",
            Message::new("gui-backup-wizard-copies", "Copies of each photo"),
            FieldKind::Choice,
        ),
        |f, n| f.choice(json!(n), &n.to_string()),
    );
    Wizard::new(
        BACKUP_WIZARD,
        &t(Message::new("gui-backup-wizard", "Set up backups")),
        SET_UP_STRATEGY_COMMAND,
    )
    .description(&t(Message::new(
        "gui-backup-wizard-about",
        "Helios keeps verified copies of every photo on your backup disks: choose how many, where, and when.",
    )))
    .step(
        "copies",
        &t(Message::new("gui-backup-wizard-step-copies", "Copies")),
        |s| {
            s.description(&t(Message::new(
                "gui-backup-wizard-copies-about",
                "Each copy is on a different disk, besides the photo in the library. Two copies survive a disk failing.",
            )))
            .field(copies.default_value(json!(2)));
        },
    )
    .step(
        "destination",
        &t(Message::new("gui-backup-wizard-step-destination", "Disks")),
        |s| {
            s.field(
                field(
                    "destination",
                    Message::new("gui-backup-wizard-destination", "Copy to"),
                    FieldKind::Choice,
                )
                .choice(
                    json!("backup"),
                    &t(Message::new(
                        "gui-backup-wizard-destination-backup",
                        "Any disk with the backup role",
                    )),
                )
                .choice(
                    json!("archive"),
                    &t(Message::new(
                        "gui-backup-wizard-destination-archive",
                        "Any disk with the archive role",
                    )),
                )
                .required()
                .help(&t(Message::new(
                    "gui-backup-wizard-destination-help",
                    "Give a disk its role on Storage › Volumes; one disk per copy.",
                ))),
            );
        },
    )
    .step(
        "off-site",
        &t(Message::new("gui-backup-wizard-step-off-site", "Off site")),
        |s| {
            s.field(
                field(
                    "offSite",
                    Message::new(
                        "gui-backup-wizard-off-site",
                        "Keep one of the copies off site",
                    ),
                    FieldKind::Toggle,
                )
                .default_value(json!(false))
                .help(&t(Message::new(
                    "gui-backup-wizard-off-site-help",
                    "A disk kept elsewhere survives a fire or a theft: mark it off site on Storage › Volumes.",
                ))),
            );
        },
    )
    .when("form.copies != '1'")
    .step(
        "when",
        &t(Message::new("gui-backup-wizard-step-when", "When")),
        |s| {
            s.description(&t(Message::new(
                "gui-backup-wizard-when-about",
                "Back up now on the Backup page makes copies at any time.",
            )))
            .field(
                field(
                    "onImport",
                    Message::new("gui-backup-wizard-on-import", "After each import"),
                    FieldKind::Toggle,
                )
                .default_value(json!(true)),
            )
            .field(
                field(
                    "onConnect",
                    Message::new(
                        "gui-backup-wizard-on-connect",
                        "When a backup disk is plugged in",
                    ),
                    FieldKind::Toggle,
                )
                .default_value(json!(true)),
            );
        },
    )
    .verb(&t(Message::new("gui-backup-wizard-finish", "Use this strategy")))
}

/// The Storage scope's volume table: every volume known
/// (`library.volumes`), toned by its state: its name edited in place, a
/// toggle per role, the off-site and RAID flags, its space as a meter,
/// when and where it was last seen, and its actions ("Mark this disk…"
/// and "Forget…" ask first).
pub fn volumes_section(
    s: &mut SectionBuilder<'_>,
    t: &dyn Fn(Message) -> String,
    forget_volume: &str,
) {
    use crate::admin::{
        MARK_VOLUME as MARK_VOLUME_COMMAND, VOLUME_ROLE as VOLUME_ROLE_COMMAND,
        VOLUME_SET as VOLUME_SET_COMMAND, VOLUMES as VOLUMES_COMMAND,
    };
    use helios_storage::Role;
    s.description(&t(Message::new(
        "gui-storage-intro",
        "The disks Helios has seen. Name them and say what each is for: Helios writes only to library, archive, backup and scratch disks.",
    )));
    s.status(Status::command(
        "volumes-summary",
        &t(Message::new("gui-settings-volumes-seen", "Volumes")),
        VOLUMES_COMMAND,
        "/summary",
    ));
    let volume = json!({ "volume": "${row.id}" });
    let set = |key: &str| json!({ "volume": "${row.id}", key: "${value}" });
    let mut table = Records::command("volumes", "", VOLUMES_COMMAND, "/volumes", "id")
        .column("name", &t(Message::new("gui-settings-volume-name", "Name")))
        .column(
            "stateText",
            &t(Message::new("gui-settings-volume-state", "State")),
        )
        .column("kind", &t(Message::new("gui-settings-volume-kind", "Kind")))
        .row_field(
            RowField::text(
                "name",
                &t(Message::new("gui-settings-volume-name", "Name")),
                VOLUME_SET_COMMAND,
                set("name"),
            )
            .help(&t(Message::new(
                "gui-storage-name-hint",
                "Its name; Enter saves it",
            ))),
        );
    for role in Role::ALL {
        let name = serde_json::to_value(role)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        table = table.row_field(RowField::toggle(
            &format!("role_{name}"),
            &t(crate::page::role_text(role)),
            VOLUME_ROLE_COMMAND,
            json!({ "volume": "${row.id}", "role": name, "on": "${value}" }),
        ));
    }
    let table = table
        .row_field(
            RowField::toggle(
                "offSite",
                &t(Message::new("gui-storage-off-site", "Off site")),
                VOLUME_SET_COMMAND,
                set("offSite"),
            )
            .help(&t(Message::new(
                "gui-storage-off-site-hint",
                "Kept away from the other disks (another building)",
            ))),
        )
        .row_field(
            RowField::toggle(
                "raid",
                &t(Message::new("gui-storage-raid", "RAID")),
                VOLUME_SET_COMMAND,
                set("raid"),
            )
            .help(&t(Message::new(
                "gui-storage-raid-note",
                "A RAID counts as one copy: it survives a dead disk, not a deletion or a theft.",
            ))),
        )
        .meter(
            "used",
            &t(Message::new("gui-settings-volume-space", "Space")),
            "space",
        )
        .column(
            "photos",
            &t(Message::new("gui-settings-volume-photos", "Photos")),
        )
        .column(
            "sourcesText",
            &t(Message::new("gui-settings-volume-sources", "Import sources")),
        )
        .column(
            "backupText",
            &t(Message::new("gui-settings-volume-backup", "Backup")),
        )
        .column(
            "seen",
            &t(Message::new("gui-settings-volume-seen", "Seen")),
        )
        .tone("tone")
        .row_action(
            Action::command(MARK_VOLUME_COMMAND)
                .title(&t(Message::new("gui-storage-mark", "Mark this disk…")))
                .args(volume.clone())
                .when("row.weak")
                .confirmation(Confirmation::new(
                    &t(Message::new(
                        "gui-storage-mark-title",
                        "Mark { $disk }?",
                    )
                    .with("disk", "${row.name}")),
                    &t(Message::new(
                        "gui-storage-mark-explain",
                        "This disk is known only by its label and size, so Helios could take another disk for it. Marking it writes a small file, .helios/volume.json, at its root: nothing else on the disk changes.",
                    )),
                    &t(Message::new(
                        "gui-storage-mark-confirm",
                        "Write the identity file",
                    )),
                )),
        )
        .row_action(
            Action::command("library.map.scanNow")
                .title(&t(Message::new("gui-settings-volume-scan", "Scan now")))
                .args(volume.clone()),
        )
        .row_action(
            Action::command(forget_volume)
                .title(&t(Message::new("gui-settings-volume-forget", "Forget…")))
                .args(volume)
                .danger()
                .confirmation(
                    Confirmation::new(
                        &t(Message::new(
                            "gui-settings-volume-forget-title",
                            "Forget what was seen on { $disk }?",
                        )
                        .with("disk", "${row.name}")),
                        &t(Message::new(
                            "gui-storage-forget-hint",
                            "Drops what the image map knows of this volume's files not in the Library; the volume stays known",
                        )),
                        &t(Message::new("gui-settings-volume-forget-verb", "Forget")),
                    )
                    .danger(),
                ),
        )
        .empty(
            &t(Message::new(
                "gui-settings-volumes-none",
                "No volume known yet: plug in a disk.",
            )),
            None,
        )
        .refresh("storage.*");
    s.table(table);
}

/// The Backup scope's strategy, edited in place: on or off, how many
/// copies, where, one off site, when copies are made (each written at
/// once through `library.setBackupStrategy`); its disks plugged in and
/// "Back up now".
pub fn strategy_section(s: &mut SectionBuilder<'_>, t: &dyn Fn(Message) -> String) {
    use crate::BACKUP_NOW as BACKUP_NOW_COMMAND;
    use crate::admin::{SET_STRATEGY as SET_STRATEGY_COMMAND, STRATEGY as BACKUP_STRATEGY_COMMAND};
    let part = |key: &str| json!({ "key": key, "value": "${value}" });
    let toggle = |key: &str, title: Message| {
        RowField::toggle(key, &t(title), SET_STRATEGY_COMMAND, part(key)).when("row.on")
    };
    let copies: Vec<(Json, String)> = (1..=5).map(|n| (json!(n), n.to_string())).collect();
    let copies: Vec<(Json, &str)> = copies
        .iter()
        .map(|(v, l)| (v.clone(), l.as_str()))
        .collect();
    s.list(
        Records::command("strategy", "", BACKUP_STRATEGY_COMMAND, "", "set")
            .titles("summaryText", None)
            .tone("tone")
            .row_field(RowField::toggle(
                "on",
                &t(Message::new(
                    "gui-storage-backup-on",
                    "Keep verified backup copies of every photo",
                )),
                SET_STRATEGY_COMMAND,
                part("on"),
            ))
            .row_field(
                RowField::choice(
                    "copies",
                    &t(Message::new("gui-settings-strategy-copies", "Copies")),
                    &copies,
                    SET_STRATEGY_COMMAND,
                    part("copies"),
                )
                .when("row.on"),
            )
            .row_field(
                RowField::choice(
                    "destination",
                    &t(Message::new("gui-settings-strategy-destinations", "On")),
                    &[],
                    SET_STRATEGY_COMMAND,
                    part("destination"),
                )
                .choices_from("destinationChoices")
                .when("row.on"),
            )
            .row_field(toggle(
                "offSite",
                Message::new("gui-storage-backup-off-site", "One of them off site"),
            ))
            .row_field(toggle(
                "onImport",
                Message::new("gui-storage-backup-on-import", "After each import"),
            ))
            .row_field(toggle(
                "onConnect",
                Message::new(
                    "gui-storage-backup-on-connect",
                    "When a backup disk is plugged in",
                ),
            ))
            .refresh("storage.*"),
    );
    s.status(Status::command(
        "strategy-online",
        &t(Message::new(
            "gui-settings-strategy-online",
            "Disks plugged in",
        )),
        BACKUP_STRATEGY_COMMAND,
        "/online",
    ));
    s.action(
        Action::command(BACKUP_NOW_COMMAND)
            .title(&t(Message::new("gui-storage-backup-now", "Back up now")))
            .primary(),
    );
    s.action(
        Action::command(egui_pop::api::admin::CMD_WIZARD_OPEN.as_str())
            .title(&t(Message::new(
                "gui-backup-wizard-open",
                "Set up backups…",
            )))
            .args(json!({ "id": BACKUP_WIZARD })),
    );
}
