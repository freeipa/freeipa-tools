/// Pluggable CI job artifact viewer.
///
/// Each CI system (PRCI, Azure, etc.) implements `CiJobViewer`.  The factory
/// function `get_viewer` inspects the job URL and returns the right implementation.
/// Adding a new CI system means implementing the trait and registering it here —
/// no changes to the TUI or data-model code are required.
pub mod prci;

use anyhow::Result;
use cursive::utils::markup::StyledString;

// ── Core types ────────────────────────────────────────────────────────────────

/// A single entry in a CI job's artifact listing.
#[derive(Debug, Clone)]
pub struct ArtifactEntry {
    /// Display name (file or directory name, not the full path).
    pub name: String,
    /// Absolute URL to this entry.
    pub url: String,
    /// True when the entry is a directory that can be navigated into.
    pub is_dir: bool,
}

// ── Trait ─────────────────────────────────────────────────────────────────────

/// Abstraction over a CI job result storage system.
///
/// Implementations must be `Send + Sync` so they can be wrapped in `Arc` and
/// shared across background threads.
pub trait CiJobViewer: Send + Sync {
    /// Returns true when this viewer knows how to handle the given job URL.
    fn can_handle(&self, url: &str) -> bool;

    /// List the artifacts available at `url`.
    /// Directories are returned with `is_dir = true`; files with `is_dir = false`.
    /// The caller is responsible for background-threading this call.
    fn list_artifacts(&self, url: &str) -> Result<Vec<ArtifactEntry>>;

    /// Fetch and render `entry` as a styled string for display in the TUI.
    /// For HTML reports this parses the content; for log files it returns plain
    /// text (decompressing gzip as needed).
    fn fetch_content(&self, entry: &ArtifactEntry) -> Result<StyledString>;

    /// Return the entry that should be selected by default when a directory is
    /// first opened (e.g. `report.html`).  Returns `None` if no preference.
    fn default_entry<'a>(&self, entries: &'a [ArtifactEntry]) -> Option<&'a ArtifactEntry>;
}

// ── Factory ───────────────────────────────────────────────────────────────────

/// Return a viewer capable of handling `url`, or `None` if no viewer matches.
pub fn get_viewer(url: &str) -> Option<Box<dyn CiJobViewer>> {
    let v = prci::PrciViewer::new();
    if v.can_handle(url) {
        return Some(Box::new(v));
    }
    // Future viewers registered here:
    // let v = azure::AzureViewer::new();
    // if v.can_handle(url) { return Some(Box::new(v)); }
    None
}
