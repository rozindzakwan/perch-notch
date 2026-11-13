//! Temporary file shelf: files dropped onto the notch are parked here so they
//! can be dragged or pasted somewhere else later.
//!
//! The shelf stores *references* (paths), never copies, so parking a 4 GB video
//! costs nothing. Entries whose file disappeared are pruned on access.

use std::path::{Path, PathBuf};

/// Stable id of a shelf entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ShelfId(pub u64);

/// One parked file or folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShelfItem {
    /// Id.
    pub id: ShelfId,
    /// Absolute path.
    pub path: PathBuf,
    /// File name for display.
    pub name: String,
    /// Whether it is a directory.
    pub is_dir: bool,
}

/// What happened when a path was offered to the shelf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShelfAdd {
    /// Added under this id.
    Added(ShelfId),
    /// Already on the shelf (moved to the front).
    AlreadyThere(ShelfId),
    /// Rejected (empty path, or the shelf is full and nothing could be evicted).
    Rejected(&'static str),
}

/// The shelf. Newest first.
#[derive(Debug, Clone)]
pub struct Shelf {
    items: Vec<ShelfItem>,
    next: u64,
    limit: usize,
}

impl Default for Shelf {
    fn default() -> Self {
        Self::new(32)
    }
}

impl Shelf {
    /// A shelf holding at most `limit` items (minimum 1).
    pub fn new(limit: usize) -> Self {
        Self {
            items: Vec::new(),
            next: 1,
            limit: limit.max(1),
        }
    }

    /// Add a path. `is_dir` is passed in so this stays free of filesystem calls
    /// and testable; the caller stats the path.
    pub fn add(&mut self, path: impl Into<PathBuf>, is_dir: bool) -> ShelfAdd {
        let path = path.into();
        if path.as_os_str().is_empty() {
            return ShelfAdd::Rejected("empty path");
        }
        if let Some(pos) = self.items.iter().position(|i| i.path == path) {
            let item = self.items.remove(pos);
            let id = item.id;
            self.items.insert(0, item);
            return ShelfAdd::AlreadyThere(id);
        }
        let id = ShelfId(self.next);
        self.next = self.next.saturating_add(1);
        let name = display_name(&path);
        self.items.insert(
            0,
            ShelfItem {
                id,
                path,
                name,
                is_dir,
            },
        );
        self.items.truncate(self.limit);
        ShelfAdd::Added(id)
    }

    /// Remove one entry.
    pub fn remove(&mut self, id: ShelfId) -> bool {
        let before = self.items.len();
        self.items.retain(|i| i.id != id);
        self.items.len() != before
    }

    /// Remove everything.
    pub fn clear(&mut self) {
        self.items.clear();
    }

    /// Look an entry up.
    pub fn get(&self, id: ShelfId) -> Option<&ShelfItem> {
        self.items.iter().find(|i| i.id == id)
    }

    /// All entries, newest first.
    pub fn items(&self) -> &[ShelfItem] {
        &self.items
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the shelf is empty.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Drop entries for which `exists` returns false (file moved or deleted).
    /// Returns how many were pruned.
    pub fn prune(&mut self, exists: impl Fn(&Path) -> bool) -> usize {
        let before = self.items.len();
        self.items.retain(|i| exists(&i.path));
        before - self.items.len()
    }

    /// Every path, for "copy all".
    pub fn paths(&self) -> Vec<PathBuf> {
        self.items.iter().map(|i| i.path.clone()).collect()
    }
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}
