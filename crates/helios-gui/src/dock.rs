//! The shell's layout nodes as egui_dock trees, and back. One dock per
//! area; the shell's layout stays the saved form, the dock is how it is
//! shown and moved.

use egui_dock::{DockState, Node as DockNode, NodeIndex, Split};
use helios_shell::layout::Node;

/// The panels of the first tabs of a node, for seeding a split.
fn first_tabs(node: &Node) -> Vec<String> {
    match node {
        Node::Tabs { panels, .. } => panels.clone(),
        Node::Split { a, .. } => first_tabs(a),
    }
}

fn place(dock: &mut DockState<String>, at: NodeIndex, node: &Node) {
    let tree = dock.main_surface_mut();
    match node {
        Node::Tabs { panels, active } => {
            if let Ok(leaf) = tree.leaf_mut(at) {
                leaf.tabs = panels.clone();
                leaf.active = (*active).min(panels.len().saturating_sub(1)).into();
            }
        }
        Node::Split {
            vertical,
            ratio,
            a,
            b,
        } => {
            // A vertical split stacks `a` above `b`; the ratio is `a`'s share.
            let side = if *vertical {
                Split::Below
            } else {
                Split::Right
            };
            let [first, second] = tree.split(at, side, *ratio, DockNode::leaf_with(first_tabs(b)));
            place(dock, first, a);
            place(dock, second, b);
        }
    }
}

/// A dock showing a layout node.
pub fn to_dock(node: &Node) -> DockState<String> {
    let mut dock = DockState::new(first_tabs(node));
    place(&mut dock, NodeIndex::root(), node);
    dock
}

fn read(dock: &DockState<String>, at: NodeIndex) -> Option<Node> {
    let tree = dock.main_surface();
    if at.0 >= tree.len() {
        return None;
    }
    match &tree[at] {
        DockNode::Empty => None,
        DockNode::Leaf(leaf) => (!leaf.tabs.is_empty()).then(|| Node::Tabs {
            panels: leaf.tabs.clone(),
            active: leaf.active.0,
        }),
        DockNode::Vertical(s) | DockNode::Horizontal(s) => {
            let vertical = tree[at].is_vertical();
            match (read(dock, at.left()), read(dock, at.right())) {
                (Some(a), Some(b)) => Some(Node::Split {
                    vertical,
                    ratio: s.fraction,
                    a: Box::new(a),
                    b: Box::new(b),
                }),
                (Some(n), None) | (None, Some(n)) => Some(n),
                (None, None) => None,
            }
        }
    }
}

/// The layout node a dock shows now; `None` when it holds no panel.
pub fn from_dock(dock: &DockState<String>) -> Option<Node> {
    read(dock, NodeIndex::root())
}
