//! The grids' thumbnail textures, within a budget: the cells drawn hold
//! theirs; beyond the budget, those drawn longest ago are freed, and asked
//! again (from the disk cache) when their cells come back.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use egui::TextureHandle;

/// The setting that limits the thumbnails' textures, in MiB.
pub const BUDGET_SETTING: &str = "system.cache.textures";

/// The textures' default budget: 512 MiB.
pub const DEFAULT_BUDGET: u64 = 512 * 1024 * 1024;

/// The textures held, by image id.
pub struct Resident {
    textures: HashMap<String, TextureHandle>,
    /// The bytes of each texture held for one image alone (a
    /// placeholder's texture is shared: not counted, never freed).
    sizes: HashMap<String, u64>,
    bytes: u64,
    budget: u64,
    /// The last pass each texture was drawn, or arrived.
    seen: RefCell<HashMap<String, u64>>,
    /// The images drawn this pass with no texture, in the order drawn.
    missing: RefCell<Vec<String>>,
    /// Asked of the pool, not arrived yet.
    requested: HashSet<String>,
}

impl Default for Resident {
    fn default() -> Self {
        Resident {
            textures: HashMap::new(),
            sizes: HashMap::new(),
            bytes: 0,
            budget: DEFAULT_BUDGET,
            seen: RefCell::default(),
            missing: RefCell::default(),
            requested: HashSet::new(),
        }
    }
}

impl Resident {
    /// The budget, in bytes.
    pub fn budget(&self) -> u64 {
        self.budget
    }

    pub fn set_budget(&mut self, bytes: u64) {
        self.budget = bytes;
    }

    /// The texture of an image, for a cell drawn at `pass`: marked seen;
    /// none is noted as missing (to ask for it).
    pub fn draw(&self, id: &str, pass: u64) -> Option<&TextureHandle> {
        let t = self.textures.get(id);
        if t.is_some() {
            if let Some(s) = self.seen.borrow_mut().get_mut(id) {
                *s = pass;
            }
        } else {
            self.missing.borrow_mut().push(id.to_string());
        }
        t
    }

    pub fn get(&self, id: &str) -> Option<&TextureHandle> {
        self.textures.get(id)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.textures.contains_key(id)
    }

    /// Whether a thumbnail of `bytes` that arrived is kept: one asked for
    /// always is, another only within the budget.
    pub fn accepts(&self, id: &str, bytes: u64) -> bool {
        self.requested.contains(id) || self.bytes + bytes <= self.budget
    }

    /// Holds an image's texture (`shared`: a placeholder's), seen at
    /// `pass`.
    pub fn insert(&mut self, id: String, texture: TextureHandle, shared: bool, pass: u64) {
        self.requested.remove(&id);
        if let Some(old) = self.sizes.remove(&id) {
            self.bytes -= old;
        }
        if !shared {
            let bytes = texture.byte_size() as u64;
            self.sizes.insert(id.clone(), bytes);
            self.bytes += bytes;
        }
        self.seen.borrow_mut().insert(id.clone(), pass);
        self.textures.insert(id, texture);
    }

    /// An arrival not kept: no longer waited for.
    pub fn dropped(&mut self, id: &str) {
        self.requested.remove(id);
    }

    /// The images drawn without a texture since the last call, not asked
    /// for yet, in the order drawn; they are marked asked.
    pub fn take_missing(&mut self) -> Vec<String> {
        let drawn = std::mem::take(&mut *self.missing.borrow_mut());
        self.ask(drawn)
    }

    /// Of `ids`, those with no texture and not asked for yet, in order;
    /// they are marked asked.
    pub fn ask(&mut self, ids: impl IntoIterator<Item = String>) -> Vec<String> {
        let mut out = Vec::new();
        for id in ids {
            if !self.textures.contains_key(&id) && self.requested.insert(id.clone()) {
                out.push(id);
            }
        }
        out
    }

    /// Forgets what was asked (a new pool replaces the last).
    pub fn clear_requests(&mut self) {
        self.requested.clear();
    }

    /// Frees textures, drawn longest ago first, until the budget is kept;
    /// never one drawn at `pass` or the pass before (the cells in view).
    /// How many were freed.
    pub fn evict(&mut self, pass: u64) -> usize {
        if self.bytes <= self.budget {
            return 0;
        }
        let mut order: Vec<(u64, String)> = {
            let seen = self.seen.borrow();
            self.sizes
                .keys()
                .map(|id| (seen.get(id).copied().unwrap_or(0), id.clone()))
                .filter(|(s, _)| s.saturating_add(1) < pass)
                .collect()
        };
        order.sort_unstable();
        let mut freed = 0;
        for (_, id) in order {
            if self.bytes <= self.budget {
                break;
            }
            if let Some(b) = self.sizes.remove(&id) {
                self.bytes -= b;
            }
            self.textures.remove(&id);
            self.seen.borrow_mut().remove(&id);
            freed += 1;
        }
        freed
    }

    /// The textures held for one image alone, and their bytes.
    pub fn usage(&self) -> (usize, u64) {
        (self.sizes.len(), self.bytes)
    }

    /// Whether this image's texture is one of `shared` (a placeholder).
    pub fn is_shared(&self, id: &str) -> bool {
        self.textures.contains_key(id) && !self.sizes.contains_key(id)
    }
}

/// The textures' budget, in bytes, from the user's settings layer (its
/// TOML text; none: the default).
///
/// # Errors
/// The layer does not read.
pub fn budget(user_layer: Option<&str>) -> Result<u64, String> {
    crate::thumbnails::setting_mib(user_layer, BUDGET_SETTING)
}
