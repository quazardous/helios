//! A workspace's arrangement: which panels sit in which area, in which
//! tabs and splits, which ones were detached into auxiliary windows and on
//! which screen. Saved per workspace in the user's scope, restored at open,
//! repaired when it no longer matches the panels or the screens.

use serde::{Deserialize, Serialize};

use crate::panels::{Area, Panels};

/// Tabs and splits of one area.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Node {
    /// Panels as tabs; `active` is the shown one.
    Tabs { panels: Vec<String>, active: usize },
    Split {
        vertical: bool,
        ratio: f32,
        a: Box<Node>,
        b: Box<Node>,
    },
}

impl Node {
    fn panels(&self) -> Vec<&str> {
        match self {
            Node::Tabs { panels, .. } => panels.iter().map(String::as_str).collect(),
            Node::Split { a, b, .. } => {
                let mut v = a.panels();
                v.extend(b.panels());
                v
            }
        }
    }

    /// Drops panels that are not in `keep`; `None` when nothing is left.
    fn retain(self, keep: &dyn Fn(&str) -> bool) -> Option<Node> {
        match self {
            Node::Tabs { panels, active } => {
                let shown = panels.get(active).cloned();
                let panels: Vec<String> = panels.into_iter().filter(|p| keep(p)).collect();
                if panels.is_empty() {
                    return None;
                }
                let active = shown
                    .and_then(|s| panels.iter().position(|p| *p == s))
                    .unwrap_or(0);
                Some(Node::Tabs { panels, active })
            }
            Node::Split {
                vertical,
                ratio,
                a,
                b,
            } => match (a.retain(keep), b.retain(keep)) {
                (Some(a), Some(b)) => Some(Node::Split {
                    vertical,
                    ratio: ratio.clamp(0.05, 0.95),
                    a: Box::new(a),
                    b: Box::new(b),
                }),
                (Some(n), None) | (None, Some(n)) => Some(n),
                (None, None) => None,
            },
        }
    }
}

/// A panel detached into its own window.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Auxiliary {
    pub panel: String,
    /// The screen it was on, by name, and its position there.
    pub screen: String,
    pub rect: [f32; 4],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    pub areas: Vec<(Area, Node)>,
    pub auxiliary: Vec<Auxiliary>,
    /// Panels the user closed: not put back as new ones are.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hidden: Vec<String>,
}

impl Layout {
    /// Every declared panel in its default area, as tabs.
    pub fn default_for(panels: &Panels) -> Layout {
        let mut areas: Vec<(Area, Node)> = Vec::new();
        for p in panels.all() {
            match areas.iter_mut().find(|(a, _)| *a == p.area) {
                Some((_, Node::Tabs { panels, .. })) => panels.push(p.id.clone()),
                _ => areas.push((
                    p.area,
                    Node::Tabs {
                        panels: vec![p.id.clone()],
                        active: 0,
                    },
                )),
            }
        }
        areas.sort_by_key(|(a, _)| *a);
        Layout {
            areas,
            auxiliary: Vec::new(),
            hidden: Vec::new(),
        }
    }

    /// Every panel placed, docked or detached.
    pub fn placed(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self.areas.iter().flat_map(|(_, n)| n.panels()).collect();
        v.extend(self.auxiliary.iter().map(|a| a.panel.as_str()));
        v
    }

    /// Brings a saved layout in line with today's panels and screens:
    /// panels no longer declared are dropped, new ones go to their default
    /// area, so do panels in an area they may no longer be in, a window on
    /// a screen that is gone comes back docked. Says what it changed.
    pub fn repair(self, panels: &Panels, screens: &[&str]) -> (Layout, Vec<String>) {
        let mut notes = Vec::new();
        let known = |id: &str| panels.get(id).is_some();
        let mut auxiliary = Vec::new();
        let mut redock = Vec::new();
        for a in self.auxiliary {
            if !known(&a.panel) {
                notes.push(format!("panel `{}` no longer exists", a.panel));
            } else if !screens.contains(&a.screen.as_str()) {
                notes.push(format!(
                    "screen `{}` is gone: `{}` docked back",
                    a.screen, a.panel
                ));
                redock.push(a.panel);
            } else {
                auxiliary.push(a);
            }
        }
        let mut areas: Vec<(Area, Node)> = Vec::new();
        for (area, node) in self.areas {
            for p in node.panels() {
                if !known(p) {
                    notes.push(format!("panel `{p}` no longer exists"));
                }
            }
            for p in node.panels() {
                if let Some(spec) = panels.get(p)
                    && !spec.allows(area)
                {
                    notes.push(format!(
                        "`{p}` cannot be in {}: moved to its default area",
                        area.name()
                    ));
                    redock.push(p.to_string());
                }
            }
            let allowed = |id: &str| panels.get(id).is_some_and(|s| s.allows(area));
            if let Some(n) = node.retain(&allowed) {
                areas.push((area, n));
            }
        }
        let hidden: Vec<String> = self.hidden.into_iter().filter(|h| known(h)).collect();
        let mut layout = Layout {
            areas,
            auxiliary,
            hidden: hidden.clone(),
        };
        let placed: Vec<String> = layout.placed().into_iter().map(str::to_string).collect();
        for p in panels.all() {
            if !placed.contains(&p.id) && !hidden.contains(&p.id) {
                if !redock.contains(&p.id) {
                    notes.push(format!("new panel `{}` placed in its default area", p.id));
                }
                match layout.areas.iter_mut().find(|(a, _)| *a == p.area) {
                    Some((_, Node::Tabs { panels, .. })) => panels.push(p.id.clone()),
                    Some((_, node)) => {
                        let old = std::mem::replace(
                            node,
                            Node::Tabs {
                                panels: vec![],
                                active: 0,
                            },
                        );
                        *node = Node::Split {
                            vertical: false,
                            ratio: 0.7,
                            a: Box::new(old),
                            b: Box::new(Node::Tabs {
                                panels: vec![p.id.clone()],
                                active: 0,
                            }),
                        };
                    }
                    None => layout.areas.push((
                        p.area,
                        Node::Tabs {
                            panels: vec![p.id.clone()],
                            active: 0,
                        },
                    )),
                }
            }
        }
        layout.areas.sort_by_key(|(a, _)| *a);
        (layout, notes)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("layouts serialize")
    }

    pub fn from_json(text: &str) -> Result<Layout, String> {
        serde_json::from_str(text).map_err(|e| format!("layout: {e}"))
    }
}
