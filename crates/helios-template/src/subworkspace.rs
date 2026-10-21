//! Sub-workspaces as data (see `docs/design/context-stack.md`,
//! "Sub-workspaces"): a sub-workspace is a named scope over nodes of the
//! workspace's own network, kept in its template as `subworkspaces`.
//! There is one network and one engine graph; a scope changes what is
//! shown and edited, never what is rendered.
//!
//! Scopes nest: a nested one names its parent, and its nodes are among
//! its parent's. Scopes under the same parent share no node, so a node
//! has one innermost scope ([`Template::scope_of`]). An id is never given
//! twice in a workspace: a deleted one is retired
//! (`retired_subworkspaces`).
//!
//! A node is named as in the network (`retouch`), or as an opened
//! composite's inner node (`retouch.mask`); a scope naming a node holds
//! its inner nodes too.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value as Json, json};

use crate::catalogue::Catalogue;
use crate::format::{NetworkDesc, SubWorkspace, Template, valid_id};

/// What an id is made of when a name gives no letter or digit.
const FALLBACK: &str = "sub";

/// Whether `node` is `scope` or inside it (`retouch.mask` is inside
/// `retouch`).
fn within(node: &str, scope: &str) -> bool {
    node == scope
        || node
            .strip_prefix(scope)
            .is_some_and(|rest| rest.starts_with('.'))
}

/// Whether `nodes` holds `node`, itself or as an inner node of one.
fn holds(nodes: &[String], node: &str) -> bool {
    nodes.iter().any(|n| within(node, n))
}

/// Whether every node of `inner` is held by `outer`.
fn holds_all(outer: &[String], inner: &[String]) -> bool {
    inner.iter().all(|n| holds(outer, n))
}

/// Whether two node lists share a node (one holding the other's).
fn overlap(a: &[String], b: &[String]) -> bool {
    a.iter().any(|n| holds(b, n)) || b.iter().any(|n| holds(a, n))
}

/// The network node a node name is, or is inside of.
fn head(node: &str) -> &str {
    node.split_once('.').map_or(node, |(h, _)| h)
}

/// A slug of a name: its letters and digits, lowercase, words joined by
/// `-` (`Skin & eyes` → `skin-eyes`).
fn slug_of(name: &str) -> String {
    let words: Vec<String> = name
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    if words.is_empty() {
        FALLBACK.into()
    } else {
        words.join("-")
    }
}

impl Template {
    /// A sub-workspace, by id.
    pub fn subworkspace(&self, id: &str) -> Option<&SubWorkspace> {
        self.subworkspaces.iter().find(|s| s.id == id)
    }

    /// The nodes of a sub-workspace, its nested ones' included; `None`
    /// for an id it does not have.
    pub fn subworkspace_nodes(&self, id: &str) -> Option<&[String]> {
        self.subworkspace(id).map(|s| s.nodes.as_slice())
    }

    /// Whether `node` is in the sub-workspace `id`, or in one nested in
    /// it.
    pub fn in_subworkspace(&self, node: &str, id: &str) -> bool {
        self.subworkspace(id).is_some_and(|s| holds(&s.nodes, node))
    }

    /// The innermost sub-workspace `node` is in; `None` at the top.
    pub fn scope_of(&self, node: &str) -> Option<&SubWorkspace> {
        self.subworkspaces
            .iter()
            .filter(|s| holds(&s.nodes, node))
            .max_by_key(|s| self.subworkspace_path(&s.id).len())
    }

    /// The sub-workspaces from the top down to `id` (`id` last): what a
    /// crumb shows. Empty for an id it does not have; a parent cycle (a
    /// file written by hand) stops the walk.
    pub fn subworkspace_path(&self, id: &str) -> Vec<&SubWorkspace> {
        let mut path = Vec::new();
        let mut next = self.subworkspace(id);
        while let Some(s) = next {
            if path.iter().any(|p: &&SubWorkspace| p.id == s.id) {
                break;
            }
            path.push(s);
            next = s.parent.as_deref().and_then(|p| self.subworkspace(p));
        }
        path.reverse();
        path
    }

    /// The sub-workspaces directly under `parent` (under the top for
    /// `None`), in order.
    pub fn subworkspace_children(&self, parent: Option<&str>) -> Vec<&SubWorkspace> {
        self.subworkspaces
            .iter()
            .filter(|s| s.parent.as_deref() == parent)
            .collect()
    }

    /// The ids taken in the workspace: those in use and those retired.
    fn subworkspace_ids(&self) -> BTreeSet<String> {
        self.subworkspaces
            .iter()
            .map(|s| s.id.clone())
            .chain(self.retired_subworkspaces.iter().cloned())
            .collect()
    }

    /// An id for a new sub-workspace named `name`: its slug, numbered
    /// (`-2`, `-3`, …) when that is in use or retired.
    pub fn free_subworkspace_id(&self, name: &str) -> String {
        free_id(name, &self.subworkspace_ids())
    }

    /// The template with a new sub-workspace `name` over `nodes`, and
    /// what it did as data (`id`, `name`, `nodes`, `parent`, `adopted`).
    /// It is nested in the innermost sub-workspace holding all of
    /// `nodes`; the sub-workspaces beside it whose nodes are all among
    /// `nodes` move inside it (`adopted`). Refused for an empty name or
    /// node list, a node the network does not have, or nodes crossing a
    /// sub-workspace's edge (some in it, some out).
    pub fn delegated(&self, name: &str, nodes: &[String]) -> Result<(Template, Json), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("name: empty".into());
        }
        if nodes.is_empty() {
            return Err("nodes: none given".into());
        }
        let mut seen = BTreeSet::new();
        for n in nodes {
            if !seen.insert(n.as_str()) {
                return Err(format!("nodes: `{n}` given twice"));
            }
            check_node(&self.network, n, None).map_err(|e| format!("nodes: {e}"))?;
        }
        // The innermost scope holding every node: the deepest of those
        // that hold them all (they are nested in one another).
        let parent = self
            .subworkspaces
            .iter()
            .filter(|s| holds_all(&s.nodes, nodes))
            .max_by_key(|s| self.subworkspace_path(&s.id).len())
            .map(|s| s.id.clone());
        let mut adopted = Vec::new();
        for s in self.subworkspace_children(parent.as_deref()) {
            if s.nodes.is_empty() || !overlap(&s.nodes, nodes) {
                continue;
            }
            if holds_all(nodes, &s.nodes) {
                adopted.push(s.id.clone());
            } else {
                return Err(format!(
                    "nodes: they cross the sub-workspace `{}` ({}): some are in it, some are not",
                    s.id, s.name
                ));
            }
        }
        let id = self.free_subworkspace_id(name);
        let mut next = self.clone();
        for s in &mut next.subworkspaces {
            if adopted.contains(&s.id) {
                s.parent = Some(id.clone());
            }
        }
        next.subworkspaces.push(SubWorkspace {
            id: id.clone(),
            name: name.into(),
            nodes: nodes.to_vec(),
            parent: parent.clone(),
        });
        let answer = json!({
            "id": id,
            "name": name,
            "nodes": nodes,
            "parent": parent,
            "adopted": adopted,
        });
        Ok((next, answer))
    }

    /// The template without the sub-workspace `id`, which is retired, and
    /// what it did as data (`id`, `name`, `parent`, `moved`): its nodes
    /// stay in the network, and the sub-workspaces nested in it move to
    /// its parent (`moved`).
    pub fn without_subworkspace(&self, id: &str) -> Result<(Template, Json), String> {
        let gone = self
            .subworkspace(id)
            .ok_or_else(|| format!("no sub-workspace `{id}`"))?
            .clone();
        let mut next = self.clone();
        next.subworkspaces.retain(|s| s.id != id);
        let mut moved = Vec::new();
        for s in &mut next.subworkspaces {
            if s.parent.as_deref() == Some(id) {
                s.parent.clone_from(&gone.parent);
                moved.push(s.id.clone());
            }
        }
        next.retire([id.to_string()]);
        let answer = json!({
            "id": id,
            "name": gone.name,
            "parent": gone.parent,
            "moved": moved,
        });
        Ok((next, answer))
    }

    /// Retires ids: never given again.
    fn retire(&mut self, ids: impl IntoIterator<Item = String>) {
        for id in ids {
            if !self.retired_subworkspaces.contains(&id) {
                self.retired_subworkspaces.push(id);
            }
        }
    }

    /// After a change from `before` (an edit, a reset): the ids
    /// `before` used or retired that are no longer in use are retired,
    /// and the nodes taken out of the network are taken out of every
    /// sub-workspace.
    pub fn follow_subworkspaces(&mut self, before: &Template) {
        let used: BTreeSet<&str> = self.subworkspaces.iter().map(|s| s.id.as_str()).collect();
        let dropped: Vec<String> = before
            .subworkspace_ids()
            .into_iter()
            .filter(|id| !used.contains(id.as_str()))
            .collect();
        self.retire(dropped);
        self.prune_nodes(&before.network);
    }

    /// Takes out of every sub-workspace the nodes `before` had that the
    /// network no longer has (a node deleted, a composite closed). A
    /// node `before` did not have either is left for the checks to name.
    fn prune_nodes(&mut self, before: &NetworkDesc) {
        let network = self.network.clone();
        for s in &mut self.subworkspaces {
            s.nodes.retain(|n| {
                check_node(before, n, None).is_err() || check_node(&network, n, None).is_ok()
            });
        }
    }

    /// Copies sub-workspaces of `from` into this template, each under a
    /// new id and its nodes renamed by `rename`; nothing links a copy to
    /// where it came from. Returns each copy's id by the source's.
    pub fn copy_subworkspaces(
        &mut self,
        from: &Template,
        rename: impl Fn(&str) -> String,
    ) -> BTreeMap<String, String> {
        let mut ids = BTreeMap::new();
        let mut taken = self.subworkspace_ids();
        // Parents first, so that a copy names its parent's copy.
        let mut order: Vec<&SubWorkspace> = from.subworkspaces.iter().collect();
        order.sort_by_key(|s| from.subworkspace_path(&s.id).len());
        for s in order {
            let id = free_id(&s.name, &taken);
            taken.insert(id.clone());
            ids.insert(s.id.clone(), id.clone());
            let mut nodes: Vec<String> = Vec::new();
            for n in &s.nodes {
                let n = rename(n);
                if !nodes.contains(&n) {
                    nodes.push(n);
                }
            }
            self.subworkspaces.push(SubWorkspace {
                id,
                name: s.name.clone(),
                nodes,
                parent: s.parent.as_ref().and_then(|p| ids.get(p).cloned()),
            });
        }
        ids
    }
}

/// An id from `name` free in `taken`.
fn free_id(name: &str, taken: &BTreeSet<String>) -> String {
    let base = slug_of(name);
    if !taken.contains(&base) {
        return base;
    }
    // `taken` has fewer ids than this range has numbers.
    (2..=taken.len() + 2)
        .map(|n| format!("{base}-{n}"))
        .find(|id| !taken.contains(id))
        .expect("more numbers than ids taken")
}

/// Whether `node` names a node of `net`: a node, or an opened
/// composite's inner node (checked against the composite's file when
/// `catalogue` is given).
fn check_node(net: &NetworkDesc, node: &str, catalogue: Option<&Catalogue>) -> Result<(), String> {
    let h = head(node);
    let n = net
        .nodes
        .iter()
        .find(|n| n.name == h)
        .ok_or_else(|| format!("no node `{h}`"))?;
    let Some(rest) = node.strip_prefix(h).and_then(|r| r.strip_prefix('.')) else {
        return Ok(());
    };
    let composite = n
        .composite
        .as_ref()
        .filter(|_| n.open)
        .ok_or_else(|| format!("`{node}`: `{h}` is not an opened composite"))?;
    if let Some(file) = catalogue.and_then(|c| c.get_composite(composite)) {
        let inner = head(rest);
        if !file.nodes.iter().any(|i| i.name == inner) {
            return Err(format!("`{node}`: `{composite}` has no node `{inner}`"));
        }
    }
    Ok(())
}

/// Whether the parents above `id` come back to one of them.
fn cycle(t: &Template, id: &str) -> bool {
    t.subworkspace_path(id)
        .first()
        .and_then(|top| top.parent.as_deref())
        .is_some_and(|p| t.subworkspace(p).is_some())
}

/// The problems of a template's sub-workspaces: ids (an id, used once,
/// not retired), names, nodes (in the network, listed once, among the
/// parent's), parents (known, no cycle), and scopes under one parent
/// sharing no node.
pub fn problems(t: &Template, catalogue: &Catalogue, p: &mut Vec<String>) {
    let mut ids = BTreeSet::new();
    for (i, s) in t.subworkspaces.iter().enumerate() {
        let at = format!("subworkspaces[{i}] ({})", s.id);
        if !valid_id(&s.id) {
            p.push(format!("{at}: `{}` is not an id", s.id));
        }
        if !ids.insert(s.id.as_str()) {
            p.push(format!("{at}: a second sub-workspace `{}`", s.id));
        }
        if t.retired_subworkspaces.contains(&s.id) {
            p.push(format!("{at}: `{}` is retired, never given again", s.id));
        }
        if s.name.trim().is_empty() {
            p.push(format!("{at}: name empty"));
        }
        let mut seen = BTreeSet::new();
        for n in &s.nodes {
            if !seen.insert(n.as_str()) {
                p.push(format!("{at}: `{n}` listed twice"));
            }
            if let Err(e) = check_node(&t.network, n, Some(catalogue)) {
                p.push(format!("{at}: {e}"));
            }
        }
        if let Some(parent) = &s.parent {
            match t.subworkspace(parent) {
                None => p.push(format!("{at}: no parent `{parent}`")),
                Some(_) if cycle(t, &s.id) => p.push(format!("{at}: its parents make a cycle")),
                Some(up) => {
                    for n in &s.nodes {
                        if !holds(&up.nodes, n) {
                            p.push(format!("{at}: `{n}` is not in its parent `{parent}`"));
                        }
                    }
                }
            }
        }
    }
    for (i, a) in t.subworkspaces.iter().enumerate() {
        for b in &t.subworkspaces[i + 1..] {
            if a.parent == b.parent && overlap(&a.nodes, &b.nodes) {
                p.push(format!(
                    "subworkspaces: `{}` and `{}` share nodes but neither is nested in the other",
                    a.id, b.id
                ));
            }
        }
    }
    for id in &t.retired_subworkspaces {
        if !valid_id(id) {
            p.push(format!("retired_subworkspaces: `{id}` is not an id"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_and_inner_nodes() {
        assert_eq!(slug_of("Skin & eyes"), "skin-eyes");
        assert_eq!(slug_of("  "), FALLBACK);
        assert_eq!(slug_of("Été 2"), "t-2");
        assert!(valid_id(&slug_of("Skin & eyes")));
        assert!(within("retouch.mask", "retouch"));
        assert!(!within("retouch2", "retouch"));
        let taken: BTreeSet<String> = ["skin".to_string(), "skin-2".to_string()].into();
        assert_eq!(free_id("Skin", &taken), "skin-3");
    }
}
