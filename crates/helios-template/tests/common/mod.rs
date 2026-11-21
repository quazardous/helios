//! What the tests share: one list of the modules the built-in templates
//! name, so a module a template starts to use is added in one place and
//! every test that loads the shipped templates sees it.

use helios_template::Catalogue;

/// The modules the built-in templates may name: every module of the
/// catalogue (`helios-modules`), as the node knows them. Composites are
/// added by each test.
pub fn modules() -> Catalogue {
    Catalogue::new().modules(helios_modules::registry())
}
