//! Shared types: scope model and explicit ChangeSet input.

use serde::{Deserialize, Serialize};

/// 1-based inclusive line range [start, end] in working-tree coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineRange {
    pub start: usize,
    pub end: usize,
}

impl LineRange {
    pub fn new(start: usize, end: usize) -> Self {
        LineRange { start, end }
    }

    /// Convert to 0-based half-open [start, end) for slicing.
    pub fn as_half_open(self) -> (usize, usize) {
        (self.start.saturating_sub(1), self.end)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Vcs {
    Git,
    Jj,
}

/// A file inside the formatting scope plus the line ranges (in working-tree
/// coordinates) that are owned by the caller's change.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopedFile {
    pub path: String,
    /// Line ranges the caller changed / wants formatted. Empty means "no
    /// ranges were detected" and the file is skipped.
    #[serde(default)]
    pub ranges: Vec<LineRange>,
    /// How many lines the caller's own change added (used by the diff-ratio
    /// gate). Unknown (explicit changeset without stats) -> None.
    #[serde(default)]
    pub agent_added_lines: Option<usize>,
}

/// The formatting scope: which VCS, which base, which files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scope {
    pub vcs: Option<Vcs>,
    pub base: String,
    pub source: String,
    pub files: Vec<ScopedFile>,
}

/// Explicit ChangeSet input (`--changeset file.json`): the caller decides the
/// scope with full control over ranges.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeSet {
    #[serde(default = "default_base")]
    pub base_ref: String,
    #[serde(default)]
    pub files: Vec<ScopedFile>,
}

fn default_base() -> String {
    "HEAD".to_string()
}
