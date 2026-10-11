//! Panels are declared, never hard-coded: a module, a plugin or the shell
//! itself says what a panel is and where it goes by default; the shell
//! places it, saves where the user moved it, and offers its commands.

use serde::{Deserialize, Serialize};

/// The shell's areas.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Area {
    /// The centre: editor groups (images, graphs), tabbed and splittable.
    Editors,
    PrimarySideBar,
    SecondarySideBar,
    BottomPanel,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PanelSpec {
    /// Stable: `helios.graph`, `helios.queue`.
    pub id: String,
    pub title: String,
    pub area: Area,
    /// The areas it may be moved to besides `area`; empty: any area.
    pub areas: Vec<Area>,
    /// Command ids the panel contributes (its toggle and focus commands
    /// are added by the shell).
    pub commands: Vec<String>,
}

impl PanelSpec {
    /// Whether the panel may live in an area.
    pub fn allows(&self, area: Area) -> bool {
        self.areas.is_empty() || area == self.area || self.areas.contains(&area)
    }
}

impl Area {
    /// As people call it.
    pub fn name(self) -> &'static str {
        match self {
            Area::Editors => "the editors",
            Area::PrimarySideBar => "the primary side bar",
            Area::SecondarySideBar => "the secondary side bar",
            Area::BottomPanel => "the bottom panel",
        }
    }
}

#[derive(Default)]
pub struct Panels {
    specs: Vec<PanelSpec>,
}

impl Panels {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn declare(&mut self, spec: PanelSpec) -> Result<(), String> {
        if self.specs.iter().any(|p| p.id == spec.id) {
            return Err(format!("panel `{}` is already declared", spec.id));
        }
        self.specs.push(spec);
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&PanelSpec> {
        self.specs.iter().find(|p| p.id == id)
    }

    pub fn all(&self) -> &[PanelSpec] {
        &self.specs
    }

    /// The commands the shell adds for each panel: focus and toggle.
    pub fn shell_commands(&self) -> Vec<(String, String)> {
        self.specs
            .iter()
            .flat_map(|p| {
                [
                    (format!("{}.focus", p.id), format!("Focus {}", p.title)),
                    (
                        format!("{}.toggle", p.id),
                        format!("Show or hide {}", p.title),
                    ),
                ]
            })
            .collect()
    }
}
