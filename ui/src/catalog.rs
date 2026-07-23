//! Catalog browsing: the synced Standard Ebooks / Project Gutenberg catalogs live
//! in SQLite (the sync is an explicit user action, never a per-search fetch), and
//! this module is the UI's read side over them — search/filter/page the rows, show
//! which ones we already imported, and queue downloads onto the background job
//! registry for the bulk "grab" flow.
//!
//! Only the shared serde types live here so far; server fns + the page component
//! land in a later pass. The types are deliberately NOT behind `cfg(ssr)` — the
//! hydrated client deserializes them out of server-fn responses.

use serde::{Deserialize, Serialize};

/// One row of a synced catalog — enough to render a result and to start a download
/// without touching the remote site again.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct CatalogEntry {
    /// "standardebooks" | "gutenberg".
    pub source: String,
    /// Stable per-source id: SE's `<author-slug>/<title-slug>`, PG's `Text#`.
    pub source_id: String,
    pub title: String,
    pub author: String,
    /// Publication year where the source actually gives one (PG's `Issued` is a
    /// release date, not a publication year, so it stays None there).
    pub year: Option<i64>,
    pub language: String,
    /// Subjects / tags, already split out of the source's "; "-separated field.
    #[serde(default)]
    pub subjects: Vec<String>,
    pub n_words: Option<i64>,
    pub reading_ease: Option<f64>,
    /// Download format we'd fetch: "epub" | "txt".
    pub fmt: String,
    /// Resolved mirror/download URL (never the human-facing page).
    pub url: String,
    /// Set when this entry is already in `books` — the UI links instead of offering
    /// a download, and the bulk grab skips it.
    pub imported_slug: Option<String>,
}

/// One page of catalog results plus the unpaged total, so the UI can show
/// "showing 48 of 1,492" without a second round trip.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct CatalogPage {
    pub total: i64,
    #[serde(default)]
    pub items: Vec<CatalogEntry>,
}

/// Freshness summary for the catalog-sync buttons: how many rows we hold per
/// source and when each was last synced (pre-formatted; the client only prints it).
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct CatalogStatus {
    pub standardebooks_rows: i64,
    pub standardebooks_synced: String,
    pub gutenberg_rows: i64,
    pub gutenberg_synced: String,
    /// How many catalog rows are already imported as books (either source).
    #[serde(default)]
    pub imported: i64,
}
