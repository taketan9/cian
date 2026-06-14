use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};

pub mod ops;

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
}

impl Entry {
    fn from_dir_entry(de: fs::DirEntry) -> Result<Self> {
        let path = de.path();
        let name = de
            .file_name()
            .into_string()
            .map_err(|raw| anyhow::anyhow!("non-utf8 filename: {:?}", raw))?;
        let is_dir = de.file_type()?.is_dir();
        Ok(Self { name, path, is_dir })
    }
}

#[derive(Debug, Clone)]
pub struct Pane {
    pub cwd: PathBuf,
    pub entries: Vec<Entry>,
    pub cursor: usize,
    /// Marked entries keyed by full path (survives reload).
    pub marks: HashSet<PathBuf>,
    /// Recently visited paths for this pane (most recent first, deduped, capped).
    pub history: Vec<PathBuf>,
}

const HISTORY_CAP: usize = 30;

impl Pane {
    pub fn new(cwd: impl Into<PathBuf>) -> Result<Self> {
        let cwd = cwd
            .into()
            .canonicalize()
            .context("invalid initial path")?;
        let mut pane = Self {
            cwd,
            entries: Vec::new(),
            cursor: 0,
            marks: HashSet::new(),
            history: Vec::new(),
        };
        pane.reload()?;
        Ok(pane)
    }

    fn push_history(&mut self, path: PathBuf) {
        self.history.retain(|p| p != &path);
        self.history.insert(0, path);
        if self.history.len() > HISTORY_CAP {
            self.history.truncate(HISTORY_CAP);
        }
    }

    pub fn reload(&mut self) -> Result<()> {
        let mut entries: Vec<Entry> = fs::read_dir(&self.cwd)
            .with_context(|| format!("read_dir failed: {}", self.cwd.display()))?
            .filter_map(|res| res.ok())
            .filter_map(|de| Entry::from_dir_entry(de).ok())
            .collect();
        entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        });
        self.entries = entries;
        if self.cursor >= self.entries.len() {
            self.cursor = self.entries.len().saturating_sub(1);
        }
        // forget marks whose path no longer exists in this directory
        let live: HashSet<PathBuf> = self.entries.iter().map(|e| e.path.clone()).collect();
        self.marks.retain(|p| live.contains(p));
        Ok(())
    }

    pub fn move_cursor(&mut self, delta: isize) {
        if self.entries.is_empty() {
            return;
        }
        let len = self.entries.len() as isize;
        let next = (self.cursor as isize + delta).clamp(0, len - 1);
        self.cursor = next as usize;
    }

    pub fn enter_selected(&mut self) -> Result<()> {
        if let Some(e) = self.entries.get(self.cursor).cloned() {
            if e.is_dir {
                let prev = self.cwd.clone();
                self.push_history(prev);
                self.cwd = e.path;
                self.cursor = 0;
                self.marks.clear();
                self.reload()?;
            }
        }
        Ok(())
    }

    pub fn go_parent(&mut self) -> Result<()> {
        let parent_owned = self.cwd.parent().map(|p| p.to_path_buf());
        if let Some(parent) = parent_owned {
            let prev = self.cwd.clone();
            self.push_history(prev);
            self.cwd = parent;
            self.cursor = 0;
            self.marks.clear();
            self.reload()?;
        }
        Ok(())
    }

    pub fn jump_to(&mut self, path: PathBuf) -> Result<()> {
        let prev = self.cwd.clone();
        self.push_history(prev);
        self.cwd = path;
        self.cursor = 0;
        self.marks.clear();
        self.reload()?;
        Ok(())
    }

    pub fn selected(&self) -> Option<&Entry> {
        self.entries.get(self.cursor)
    }

    pub fn toggle_mark_at(&mut self, idx: usize) {
        if let Some(e) = self.entries.get(idx) {
            let p = e.path.clone();
            if !self.marks.remove(&p) {
                self.marks.insert(p);
            }
        }
    }

    pub fn set_mark_at(&mut self, idx: usize) {
        if let Some(e) = self.entries.get(idx) {
            self.marks.insert(e.path.clone());
        }
    }

    pub fn is_marked(&self, idx: usize) -> bool {
        self.entries
            .get(idx)
            .map(|e| self.marks.contains(&e.path))
            .unwrap_or(false)
    }

    pub fn clear_marks(&mut self) {
        self.marks.clear();
    }

    pub fn mark_count(&self) -> usize {
        self.marks.len()
    }

    /// Return marked paths, or if none marked, the cursor's path as a fallback.
    pub fn target_paths(&self) -> Vec<PathBuf> {
        if !self.marks.is_empty() {
            let mut v: Vec<PathBuf> = self.marks.iter().cloned().collect();
            v.sort();
            v
        } else if let Some(e) = self.selected() {
            vec![e.path.clone()]
        } else {
            Vec::new()
        }
    }
}
