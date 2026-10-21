//! What Helios ships: the built-in templates and the composite files,
//! as data built into the program.

use crate::bundle::Bundle;
use crate::format::CompositeFile;

/// The built-in templates, each as its bundle's files.
const BUILTIN: &[&[(&str, &str)]] = &[
    &[
        (
            "template.json",
            include_str!("../../../templates/portrait-retouch.helios-template/template.json"),
        ),
        (
            "layout.json",
            include_str!("../../../templates/portrait-retouch.helios-template/layout.json"),
        ),
        (
            "i18n/en/template.ftl",
            include_str!(
                "../../../templates/portrait-retouch.helios-template/i18n/en/template.ftl"
            ),
        ),
        (
            "i18n/fr/template.ftl",
            include_str!(
                "../../../templates/portrait-retouch.helios-template/i18n/fr/template.ftl"
            ),
        ),
    ],
    &[
        (
            "template.json",
            include_str!("../../../templates/import-cull-dedupe.helios-template/template.json"),
        ),
        (
            "layout.json",
            include_str!("../../../templates/import-cull-dedupe.helios-template/layout.json"),
        ),
        (
            "i18n/en/template.ftl",
            include_str!(
                "../../../templates/import-cull-dedupe.helios-template/i18n/en/template.ftl"
            ),
        ),
        (
            "i18n/fr/template.ftl",
            include_str!(
                "../../../templates/import-cull-dedupe.helios-template/i18n/fr/template.ftl"
            ),
        ),
    ],
    &[
        (
            "template.json",
            include_str!("../../../templates/develop-preview.helios-template/template.json"),
        ),
        (
            "layout.json",
            include_str!("../../../templates/develop-preview.helios-template/layout.json"),
        ),
        (
            "i18n/en/template.ftl",
            include_str!("../../../templates/develop-preview.helios-template/i18n/en/template.ftl"),
        ),
        (
            "i18n/fr/template.ftl",
            include_str!("../../../templates/develop-preview.helios-template/i18n/fr/template.ftl"),
        ),
    ],
];

/// The composite files Helios ships.
const COMPOSITES: &[&str] = &[include_str!(
    "../../../templates/composites/helios.masked-adjustment@1.helios-composite.json"
)];

/// The built-in templates.
pub fn templates() -> Vec<Bundle> {
    BUILTIN
        .iter()
        .map(|files| Bundle::from_files(files).expect("the built-in templates read"))
        .collect()
}

/// The composite files Helios ships.
pub fn composites() -> Vec<CompositeFile> {
    COMPOSITES
        .iter()
        .map(|text| crate::bundle::parse_composite(text).expect("the shipped composites read"))
        .collect()
}
