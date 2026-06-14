use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};

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
}

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
        };
        pane.reload()?;
        Ok(pane)
    }

    pub fn reload(&mut self) -> Result<()> {
        let mut entries: Vec<Entry> = fs::read_dir(&self.cwd)
            .with_context(|| format!("read_dir failed: {}", self.cwd.display()))?
            .filter_map(|res| res.ok())
            .filter_map(|de| Entry::from_dir_entry(de).ok())
            .collect();
        // directories first, then case-insensitive alphabetical
        entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        });
        self.entries = entries;
        if self.cursor >= self.entries.len() {
            self.cursor = self.entries.len().saturating_sub(1);
        }
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
                self.cwd = e.path;
                self.cursor = 0;
                self.reload()?;
            }
        }
        Ok(())
    }

    pub fn go_parent(&mut self) -> Result<()> {
        if let Some(parent) = self.cwd.parent() {
            self.cwd = parent.to_path_buf();
            self.cursor = 0;
            self.reload()?;
        }
        Ok(())
    }

    pub fn selected(&self) -> Option<&Entry> {
        self.entries.get(self.cursor)
    }
}
