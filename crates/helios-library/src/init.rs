//! The library's init steps, declared here (`helios_init`): the library
//! folder, which import requires before it runs. The composition root
//! gathers this [`SET`] with the other crates' sets
//! (`helios_node_core::init::SETS`).

use helios_core::Message;
use helios_init::{Set, Step};

use crate::health::{CHOOSE_LIBRARY_FOLDER, FOLDER_CHOSEN};

/// The library module, owner of the library folder step.
pub const OWNER: &str = "library";

/// The library folder step: where imports copy photos. Its id is the
/// health check's ([`FOLDER_CHOSEN`]), whose fix fills it.
pub const LIBRARY_FOLDER: &str = FOLDER_CHOSEN;

/// The setting the library folder step fills: the library folder's path.
pub const LIBRARY_FOLDER_SETTING: &str = "library.folder";

/// The import module, which requires the library folder.
pub const IMPORT: &str = "import";

/// The library's steps: the library folder, filled on a desktop by its
/// command ([`CHOOSE_LIBRARY_FOLDER`], with a `path`, or the setup opened
/// on the step), the health warning's fix too.
pub fn steps() -> Vec<Step> {
    vec![
        Step::new(
            LIBRARY_FOLDER,
            Message::new("init-library-folder", "Library folder"),
            Message::new(
                "init-library-folder-why",
                "Imports copy photos into it, and picked photos go there.",
            ),
        )
        .fills(&[LIBRARY_FOLDER_SETTING])
        .command(CHOOSE_LIBRARY_FOLDER),
    ]
}

/// The library's init declarations: its steps, and import's requirement.
pub const SET: Set = Set {
    owner: OWNER,
    steps,
    requires: &[(IMPORT, &[LIBRARY_FOLDER])],
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_library_steps_are_sound() {
        for s in steps() {
            assert_eq!(s.problems(), Vec::<String>::new(), "{}", s.id);
        }
    }
}
