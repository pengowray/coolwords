use std::collections::{HashMap, HashSet};
#[cfg(feature = "ssr")]
use std::collections::BTreeSet;

use leptos::prelude::*;
use leptos_meta::{provide_meta_context, MetaTags, Stylesheet, Title};
use leptos_router::components::{Route, Router, Routes, A};
use leptos_router::hooks::{query_signal, query_signal_with_options, use_location, use_navigate};
use leptos_router::{NavigateOptions, StaticSegment};
use serde::{Deserialize, Serialize};

/// Home Assistant ingress serves the whole app under a per-session path prefix
/// (e.g. `/api/hassio_ingress/<token>`). HA strips that prefix before forwarding the
/// request to us and passes it back in the `X-Ingress-Path` header. To make asset
/// URLs, router links, and server-function calls resolve against that prefix instead
/// of the HA root, we thread it through the app as a "base". It is the empty string
/// for normal (non-ingress) access, so every `format!("{base}/…")` below reduces to
/// the original absolute path and behaviour is unchanged off ingress.
#[derive(Clone)]
pub struct BasePath(pub String);

/// Resolve the ingress base (no trailing slash), or "" when not behind ingress.
/// On the server we read the `X-Ingress-Path` request header; in the browser we read
/// the `data-base` attribute the server stamped onto `<html>` in [`shell`].
pub fn ingress_base() -> String {
    #[cfg(feature = "ssr")]
    {
        use_context::<axum::http::request::Parts>()
            .and_then(|p| {
                p.headers
                    .get("x-ingress-path")
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.trim_end_matches('/').to_string())
            })
            .unwrap_or_default()
    }
    #[cfg(not(feature = "ssr"))]
    {
        web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.document_element())
            .and_then(|e| e.get_attribute("data-base"))
            .map(|s| s.trim_end_matches('/').to_string())
            .unwrap_or_default()
    }
}

/// The ingress base provided by [`App`] via context ("" when not behind ingress).
/// Prefix internal paths with this before use: leptos_router leaves absolute hrefs
/// (`/books`) untouched, so the base must be applied by hand.
fn base_path() -> String {
    use_context::<BasePath>().map(|b| b.0).unwrap_or_default()
}

/// A tag definition in the user's collection (builtin defaults + custom tags).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TagDef {
    pub name: String,
    pub comment: Option<String>,
    pub builtin: bool,
    /// 'book' (this book only) | 'word' (the word across all books).
    pub scope: String,
    /// 'interesting' (favourites) | 'neutral' (note) | 'uninteresting' (negative).
    pub interest: String,
    pub sort: i64,
    /// User subheading within a scope ('' = ungrouped). Tags with the same section
    /// (kept contiguous by `sort`) render under one subheading.
    pub section: String,
    /// 'bool' (a plain on/off tag) | 'scale' (a 1..scale_max rating).
    pub kind: String,
    /// Top of the scale (bool tags are 1; scales are 2..=10).
    pub scale_max: i64,
    /// Optional per-level names as a JSON array (index 1..=scale_max), or None.
    pub scale_labels: Option<String>,
}

impl TagDef {
    /// True if this is a graded (1..scale_max) tag rather than a plain boolean.
    pub fn is_scale(&self) -> bool {
        self.kind == "scale" && self.scale_max > 1
    }
    /// The clamped ceiling used for a scale (defensive against bad stored data).
    pub fn max_level(&self) -> i32 {
        if self.is_scale() { self.scale_max.clamp(2, 10) as i32 } else { 1 }
    }
}

/// Normalize a free-text tag name to its canonical collection form, or None if
/// it isn't a usable tag. Pure (client + server) so optimistic UI matches storage.
///
/// A '.' nests the tag under its prefix (`thing.material` is a child of `thing`);
/// each dotted segment is sanitized like a standalone name (letters required,
/// 1..30 chars). At most 4 levels deep. `star` and `pick` can't head a hierarchy.
pub fn sanitize_tag(name: &str) -> Option<String> {
    let lowered = name.trim().to_lowercase();
    let raw_segs: Vec<&str> = lowered.split('.').collect();
    if !(1..=4).contains(&raw_segs.len()) {
        return None;
    }
    let mut segs: Vec<String> = Vec::with_capacity(raw_segs.len());
    for raw in raw_segs {
        let kept: String = raw
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == ' ' || *c == '-')
            .collect();
        let seg = kept.split_whitespace().collect::<Vec<_>>().join(" ");
        if !(1..=30).contains(&seg.chars().count()) || !seg.chars().any(|c| c.is_ascii_alphabetic())
        {
            return None;
        }
        segs.push(seg);
    }
    // Reserved heads: `pick:` contextual tags, and `star` (the singleton ★ quick
    // favourite — no children, or a child would imply-favourite through the ★ path).
    if segs[0] == "pick" || (segs[0] == "star" && segs.len() > 1) {
        return None;
    }
    Some(segs.join("."))
}

/// Is `anc` a proper ancestor of `name` in the dotted hierarchy (`thing` of
/// `thing.material`)? Pure (client + server).
pub fn is_ancestor(anc: &str, name: &str) -> bool {
    name.len() > anc.len() && name.starts_with(anc) && name.as_bytes()[anc.len()] == b'.'
}

/// Every proper-ancestor name of a dotted tag, outermost first:
/// `"a.b.c"` -> `["a", "a.b"]`. Empty for top-level or `pick:` tags. Pure.
pub fn ancestor_names(name: &str) -> Vec<String> {
    if name.starts_with("pick:") || !name.contains('.') {
        return Vec::new();
    }
    let segs: Vec<&str> = name.split('.').collect();
    (1..segs.len()).map(|i| segs[..i].join(".")).collect()
}

/// Normalize a subheading label: trim, collapse internal whitespace, keep only
/// alphanumerics/space/dash, cap at 40 chars. Case is preserved (headings read
/// better cased). Empty string is valid and means "ungrouped". Pure (client+server).
pub fn sanitize_section(s: &str) -> String {
    let kept: String = s
        .trim()
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-')
        .collect();
    kept.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(40).collect()
}

/// A tag value is allowed if it's a contextual `pick:<bucket>` tag or a clean
/// collection name (set_tag auto-registers brand-new collection tags).
#[cfg(feature = "ssr")]
fn tag_allowed(tag: &str) -> bool {
    if tag.starts_with("pick:") {
        return (6..40).contains(&tag.len())
            && tag[5..].chars().all(|c| c.is_ascii_alphanumeric() || c == '.');
    }
    sanitize_tag(tag).as_deref() == Some(tag)
}

/// The two tag scopes and three interest levels (validated server-side).
pub const SCOPE_BOOK: &str = "book";
pub const SCOPE_WORD: &str = "word";
#[cfg(feature = "ssr")]
fn valid_scope(s: &str) -> bool {
    s == SCOPE_BOOK || s == SCOPE_WORD
}
#[cfg(feature = "ssr")]
fn valid_interest(s: &str) -> bool {
    matches!(s, "interesting" | "neutral" | "uninteresting")
}

/// Shared client-side tag state + the persistence actions + the tag collection,
/// passed via context.
#[derive(Clone, Copy)]
pub struct Tagger {
    /// Per-(book,word) applied tags with their tri-state value: absent key = never
    /// considered; value 0 = considered & declined (remembered, not applied);
    /// value >=1 = applied (the magnitude, for scale tags).
    pub store: RwSignal<HashMap<(i64, i64), HashMap<String, i32>>>,
    pub action: ServerAction<SetTag>,
    /// Set a tag's tri-state value on a word (scale ratings + the 0/considered state).
    pub set_val: ServerAction<SetTagValue>,
    pub tags: RwSignal<Vec<TagDef>>,
    pub add: ServerAction<AddTag>,
    /// Create a tag with an explicit scope + interest (picker / manager adders).
    pub create: ServerAction<CreateTag>,
    /// Atomically create a tag AND apply it to a word (the picker's add path) —
    /// race-free vs the auto-register in `set_tag`.
    pub create_apply: ServerAction<CreateAndApplyTag>,
    // tag-collection mutations (editor + drag); the picker/editor refetch the
    // collection (and book tags) whenever any of their versions change.
    pub scope: ServerAction<SetTagScope>,
    pub interest: ServerAction<SetTagInterest>,
    /// Convert a tag between bool and 1..N scale (manager only).
    pub set_scale: ServerAction<SetTagScale>,
    pub rename: ServerAction<RenameTag>,
    pub del: ServerAction<DeleteTag>,
    /// Drag-drop reorder + section reassignment within a scope.
    pub layout: ServerAction<SetScopeLayout>,
}

/// Combined version of every tag-collection mutation — Resources watch this to
/// refetch `list_tags` / `book_tags` after any edit.
fn tagger_rev(t: Tagger) -> usize {
    t.add.version().get()
        + t.create.version().get()
        + t.create_apply.version().get()
        + t.scope.version().get()
        + t.interest.version().get()
        + t.set_scale.version().get()
        + t.rename.version().get()
        + t.del.version().get()
        + t.layout.version().get()
}

/// Version of just the mutations that change *applications* (word_tags rows), so
/// the per-book tag store refetches on scope move / rename / delete but not on a
/// plain add (whose application arrives via its own optimistic SetTag).
fn tagger_apps_rev(t: Tagger) -> usize {
    t.scope.version().get() + t.rename.version().get() + t.del.version().get()
        + t.create_apply.version().get()
}

/// Interest class of a tag (drives favourite vs note vs negative). `pick:` contextual
/// tags and unknown names are neutral.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Interest {
    Interesting,
    Neutral,
    Uninteresting,
}

fn tag_interest(t: Tagger, name: &str) -> Interest {
    if name.starts_with("pick:") {
        return Interest::Neutral;
    }
    t.tags.with(|v| {
        v.iter().find(|d| d.name == name).map(|d| match d.interest.as_str() {
            "uninteresting" => Interest::Uninteresting,
            "neutral" => Interest::Neutral,
            _ => Interest::Interesting,
        })
    })
    .unwrap_or(Interest::Interesting)
}

/// Interest of a tag ONLY if it's a real member of the collection. Dotted ancestors
/// that don't exist as their own tag contribute nothing (avoids the `tag_interest`
/// default of Interesting manufacturing phantom favourites through the hierarchy).
fn known_interest(t: Tagger, name: &str) -> Option<Interest> {
    if name.starts_with("pick:") {
        return None;
    }
    t.tags.with(|v| {
        v.iter().find(|d| d.name == name).map(|d| match d.interest.as_str() {
            "uninteresting" => Interest::Uninteresting,
            "neutral" => Interest::Neutral,
            _ => Interest::Interesting,
        })
    })
}

/// Fold one tag's interest into the running effective interest: interesting wins
/// over uninteresting wins over neutral.
fn fold_interest(acc: &mut Option<Interest>, i: Interest) {
    match i {
        Interest::Interesting => *acc = Some(Interest::Interesting),
        Interest::Uninteresting if *acc != Some(Interest::Interesting) => {
            *acc = Some(Interest::Uninteresting)
        }
        Interest::Neutral if acc.is_none() => *acc = Some(Interest::Neutral),
        _ => {}
    }
}

/// A word's effective interest from its applied tags (value >= 1; a 0/considered
/// row never contributes). A tag also lends its existing ancestors' interest, so an
/// applied child implies its parent for favouriting. None if it carries no applied tags.
fn word_interest(t: Tagger, key: (i64, i64)) -> Option<Interest> {
    t.store.with(|m| {
        let map = m.get(&key)?;
        let mut acc: Option<Interest> = None;
        for (tag, &val) in map {
            if val < 1 {
                continue;
            }
            fold_interest(&mut acc, tag_interest(t, tag));
            for anc in ancestor_names(tag) {
                if let Some(i) = known_interest(t, &anc) {
                    fold_interest(&mut acc, i);
                }
            }
        }
        acc
    })
}

/// Effective interest across a merged family (any in-book member).
fn group_interest(t: Tagger, book_id: i64, members: &[i64]) -> Option<Interest> {
    let mut acc: Option<Interest> = None;
    for &w in members {
        match word_interest(t, (book_id, w)) {
            Some(Interest::Interesting) => return Some(Interest::Interesting),
            Some(Interest::Uninteresting) => acc = Some(Interest::Uninteresting),
            Some(Interest::Neutral) => acc = acc.or(Some(Interest::Neutral)),
            None => {}
        }
    }
    acc
}

/// The distinct collection tags applied anywhere in a merged family, each paired
/// with its interest. `pick:` contextual buckets are excluded (they're not the
/// user's own labels), so the card strip only surfaces real tags. Sorted
/// interesting → neutral → uninteresting, then by name, for a stable display.
fn group_tags(t: Tagger, book_id: i64, members: &[i64]) -> Vec<(String, Interest)> {
    let mut names: HashSet<String> = HashSet::new();
    t.store.with(|m| {
        for &w in members {
            if let Some(set) = m.get(&(book_id, w)) {
                // Only directly-applied tags (value >= 1); implied parents are NOT
                // listed here (they'd double-count against the child in the pills).
                for (tag, &val) in set {
                    if val >= 1 && !tag.starts_with("pick:") {
                        names.insert(tag.clone());
                    }
                }
            }
        }
    });
    let mut out: Vec<(String, Interest)> =
        names.into_iter().map(|n| { let i = tag_interest(t, &n); (n, i) }).collect();
    let rank = |i: &Interest| match i {
        Interest::Interesting => 0,
        Interest::Neutral => 1,
        Interest::Uninteresting => 2,
    };
    out.sort_by(|a, b| rank(&a.1).cmp(&rank(&b.1)).then_with(|| a.0.cmp(&b.0)));
    out
}

/// Per-interest counts of a family's applied collection tags: (interesting,
/// neutral, uninteresting). Used for the compact count pills when a word carries
/// too many tags to list as chips.
fn group_tag_counts(t: Tagger, book_id: i64, members: &[i64]) -> (usize, usize, usize) {
    let mut c = (0usize, 0usize, 0usize);
    for (_, i) in group_tags(t, book_id, members) {
        match i {
            Interest::Interesting => c.0 += 1,
            Interest::Neutral => c.1 += 1,
            Interest::Uninteresting => c.2 += 1,
        }
    }
    c
}

pub fn shell(options: LeptosOptions) -> impl IntoView {
    // Behind ingress this is the path prefix; "" otherwise. Stamped onto <html> so the
    // hydrating wasm can recover it (no request headers on the client), and passed to
    // HydrationScripts as `root` so the /pkg JS+WASM load from under the prefix.
    let base = ingress_base();
    view! {
        <!DOCTYPE html>
        <html lang="en" attr:data-base=base.clone()>
            <head>
                <meta charset="utf-8"/>
                <meta name="viewport" content="width=device-width, initial-scale=1"/>
                <AutoReload options=options.clone() />
                <HydrationScripts options root=base.clone()/>
                <MetaTags/>
            </head>
            <body>
                <App/>
            </body>
        </html>
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Book {
    pub id: i64,
    pub title: String,
    pub n_selected: i64,
}

/// One tagged word on the cross-book Collection page: which of the user's tags it
/// carries (each with its interest, for colouring) and which books it appears in.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CollectionEntry {
    pub word: String,
    /// Dictionary word id, for linking into a book's word detail (0 if unresolved).
    pub word_id: i64,
    pub gloss: Option<String>,
    /// Effective interest ("interesting" | "neutral" | "uninteresting"), for sort/section.
    pub interest: String,
    /// Applied collection tags: (name, interest). `pick:` buckets are excluded.
    pub tags: Vec<(String, String)>,
    /// Books this word (with these tags) reaches: (book_id, title).
    pub books: Vec<(i64, String)>,
    /// Ranking number for a "smart" collection (e.g. net favourite − negative
    /// all-books tags), shown as a badge. `None` for an ordinary tag/all view.
    pub metric: Option<i64>,
}

/// One entry in the filter dropdown. `value` is what goes in the `cat` query
/// param; `group` lets the UI break entries into <optgroup>s. Values use a small
/// namespace: `pos:noun`, a raw lexname like `noun.animal`, `origin:uncommon`/`origin:rare`, or
/// `era:of` / `era:before` / `era:old` / `era:timeless`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FilterOpt {
    pub value: String,
    pub label: String,
    pub count: i64,
    pub group: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Candidate {
    pub word_id: i64,
    pub word: String,
    pub in_book: i64,
    pub score: f64,
    pub gloss: Option<String>,
    pub origin_code: Option<String>,
    pub origin_name: Option<String>,
    pub category: Option<String>,
    pub cluster: Option<i64>,
    pub selected: bool,
    pub example: Option<String>,
    pub tags: Vec<String>,
    pub buckets: Vec<String>,
    /// Distinct in-book surface forms merged into this group (1 = no merge).
    pub n_forms: i64,
    /// In-book member word_ids of this group (incl. the representative), so tag
    /// state can be unioned across the family for cross-level visibility.
    pub members: Vec<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RelTarget {
    pub rel: String,
    pub target: String,
    pub target_word_id: Option<i64>,
    pub in_book: bool,
}

/// The resolved root/lemma of a headword (e.g. "harpoon" for "harpooneer"),
/// shown as a second usage-over-time chart with its own category.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RootInfo {
    pub word: String,
    pub word_id: i64,
    pub freq_pm: Option<f64>,
    pub category: Option<String>,
    pub trajectory: Vec<(i32, f64)>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WordInfo {
    pub word_id: i64,
    pub word: String,
    pub gloss: Option<String>,
    pub origin_code: Option<String>,
    pub origin_name: Option<String>,
    pub freq_pm: Option<f64>,
    pub syllables: Option<i64>,
    pub in_book: i64,
    pub example: Option<String>,
    pub book_year: Option<i64>,
    pub categories: Vec<String>,
    pub buckets: Vec<String>,
    pub base: Option<(String, f64)>,
    /// In-book members of this word's group at the chosen level: (word_id, word,
    /// in-book count, overall freq_pm). Each is individually taggable in the UI.
    pub family: Vec<(i64, String, i64, f64)>,
    pub relations: Vec<RelTarget>,
    pub trajectory: Vec<(i32, f64)>,
    /// Era label (relative to the book's year): "ahead of its time" / "of its
    /// time" / "declining" / "timeless" / "always rare".
    pub era: Option<String>,
    /// Present-day status, orthogonal to `era`: effectively extinct today.
    pub obsolete: bool,
    /// Root/lemma word (when distinct), for a second usage chart.
    pub root: Option<RootInfo>,
    /// Other books (≠ current) containing this word or a level-family member:
    /// (book_id, title, combined in-book count). Shows the reach of a global tag.
    pub also_in: Vec<(i64, String, i64)>,
}

/// One labelled span of an imported file: kept body text vs stripped boilerplate
/// (Gutenberg header/licence, table of contents, EPUB front-matter, ...). Mirrors
/// the JSON emitted by `python -m ingest.import_book`.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct ImportSegment {
    pub label: String,
    pub kept: bool,
    pub note: String,
    pub char_len: i64,
    pub preview: String,
    pub truncated: bool,
}

/// Result of inspecting a dropped (but not yet committed) file: detected metadata,
/// duplicate status, and the kept/stripped segmentation for the review viewer.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Inspection {
    #[serde(default)]
    pub token: String, // staging filename, echoed back to confirm_import (server-set)
    #[serde(default)]
    pub format: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub year: Option<i64>,
    #[serde(default)]
    pub year_note: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub source_id: String,
    #[serde(default)]
    pub content_hash: String,
    #[serde(default)]
    pub n_tokens: i64,
    #[serde(default)]
    pub n_types: i64,
    #[serde(default)]
    pub duplicate_of: Option<String>,
    #[serde(default)]
    pub duplicate_title: Option<String>,
    #[serde(default)]
    pub suggested_slug: String,
    #[serde(default)]
    pub orig_filename: String,
    #[serde(default)]
    pub segments: Vec<ImportSegment>,
    // PDF-only blocks (absent/empty for txt/epub).
    #[serde(default)]
    pub pdf: Option<PdfInfo>,
    #[serde(default)]
    pub needs_ocr: bool,
    #[serde(default)]
    pub ocr: Option<OcrInfo>,
}

/// Page statistics for a dropped PDF.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct PdfInfo {
    #[serde(default)]
    pub n_pages: i64,
    #[serde(default)]
    pub n_text_pages: i64,
    #[serde(default)]
    pub n_image_pages: i64,
    #[serde(default)]
    pub has_text_layer: bool,
}

/// OCR engine availability for the import UI.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct OcrInfo {
    #[serde(default)]
    pub engines: HashMap<String, OcrEngine>,
    #[serde(default)]
    pub default_engine: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct OcrEngine {
    #[serde(default)]
    pub available: bool,
    #[serde(default)]
    pub detail: String,
}

/// Result of `--ocr-compare`: per-page embedded-vs-OCR diffs. (Named …Result to
/// avoid colliding with the `OcrCompare` args struct the #[server] macro derives
/// from the `ocr_compare` server fn.)
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct OcrCompareResult {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub engine: String,
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub pages: Vec<OcrPage>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct OcrPage {
    #[serde(default)]
    pub page: i64,
    #[serde(default)]
    pub sim: f64,
    #[serde(default)]
    pub embedded_words: i64,
    #[serde(default)]
    pub ocr_words: i64,
    #[serde(default)]
    pub ops: Vec<DiffOp>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct DiffOp {
    #[serde(default)]
    pub op: String, // eq | gap | del | ins | rep
    #[serde(default)]
    pub a: String,
    #[serde(default)]
    pub b: String,
}

/// Result of committing an import (book ingested + analysis pipeline run).
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct ImportResult {
    #[serde(default)]
    pub slug: String,
    #[serde(default)]
    pub book_id: i64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub n_tokens: i64,
    #[serde(default)]
    pub n_types: i64,
    #[serde(default)]
    pub candidates: i64,
}

/// A background job's serializable progress snapshot (polled by the manage page).
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct JobProgress {
    pub id: String,
    pub kind: String,   // "ocr" | "reingest" | "trajectory"
    pub book_id: i64,
    pub tag: String,    // engine (ocr) / source (reingest) — for dedup + display
    pub status: String, // "queued" | "running" | "done" | "failed" | "cancelled"
    pub percent: f32,   // -1 = indeterminate
    pub message: String,
    pub updated: u64,
}

/// One book row for the management page.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct BookAdmin {
    pub id: i64,
    pub slug: String,
    pub title: String,
    pub author: String,
    pub year: Option<i64>,
    pub format: String,
    pub source: String,
    pub text_source: String,
    pub n_tokens: i64,
    pub n_types: i64,
    pub n_selected: i64,
    pub ingested_at: String,
}

/// PDF OCR / text-source state for a book (from `--ocr-status`). Named …OcrStatus
/// to avoid the `BookOcrStatus` args struct the #[server] macro derives from the
/// `book_ocr_status` server fn.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct OcrStatus {
    #[serde(default)]
    pub is_pdf: bool,
    #[serde(default)]
    pub text_source: Option<String>,
    #[serde(default)]
    pub n_pages: i64,
    #[serde(default)]
    pub n_text_pages: i64,
    #[serde(default)]
    pub has_text_layer: bool,
    #[serde(default)]
    pub default_engine: String,
    #[serde(default)]
    pub engines: HashMap<String, OcrEngineStatus>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct OcrEngineStatus {
    #[serde(default)]
    pub available: bool,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub cached_pages: i64,
    #[serde(default)]
    pub complete: bool,
}

#[cfg(feature = "ssr")]
fn db_path() -> String {
    if let Ok(p) = std::env::var("COOLWORDS_DB") {
        return p;
    }
    for p in ["../data/coolwords.db", "data/coolwords.db"] {
        if std::path::Path::new(p).exists() {
            return p.to_string();
        }
    }
    "../data/coolwords.db".to_string()
}

/// Path to the per-user database (tags + collection). Self-contained; overridable.
#[cfg(feature = "ssr")]
fn user_db_path() -> String {
    if let Ok(p) = std::env::var("COOLWORDS_USER_DB") {
        return p;
    }
    for p in ["../data/user.db", "data/user.db"] {
        if std::path::Path::new(p).exists() {
            return p.to_string();
        }
    }
    "../data/user.db".to_string()
}

#[cfg(feature = "ssr")]
const USER_SCHEMA: &str = include_str!("../../schema/user.sql");

/// Additively add columns that postdate the original `tags` table (CREATE TABLE
/// IF NOT EXISTS won't add them to an existing DB). Mirrors ingest/userdb.py.
#[cfg(feature = "ssr")]
fn migrate_user(u: &rusqlite::Connection) -> Result<(), ServerFnError> {
    // Columns of `tags` and `word_tags` that postdate their original CREATE TABLE.
    let cols = |table: &str| -> Result<HashSet<String>, ServerFnError> {
        let mut have = HashSet::new();
        let mut s = u.prepare(&format!("PRAGMA table_info({table})"))
            .map_err(|e| ServerFnError::new(e.to_string()))?;
        let rows = s.query_map([], |r| r.get::<_, String>(1))
            .map_err(|e| ServerFnError::new(e.to_string()))?;
        for r in rows.filter_map(Result::ok) {
            have.insert(r);
        }
        Ok(have)
    };
    let have = cols("tags")?;
    for (name, decl) in [("scope", "TEXT NOT NULL DEFAULT 'book'"),
                         ("interest", "TEXT NOT NULL DEFAULT 'interesting'"),
                         ("section", "TEXT NOT NULL DEFAULT ''"),
                         ("kind", "TEXT NOT NULL DEFAULT 'bool'"),
                         ("scale_max", "INTEGER NOT NULL DEFAULT 1"),
                         ("scale_labels", "TEXT")] {
        if !have.contains(name) {
            u.execute(&format!("ALTER TABLE tags ADD COLUMN {name} {decl}"), [])
                .map_err(|e| ServerFnError::new(e.to_string()))?;
        }
    }
    // `value` on word_tags is the tri-state rating (NULL==1==applied). Nullable, no
    // default, so legacy rows read as applied.
    let have_apps = cols("word_tags")?;
    if !have_apps.contains("value") {
        u.execute("ALTER TABLE word_tags ADD COLUMN value INTEGER", [])
            .map_err(|e| ServerFnError::new(e.to_string()))?;
    }
    Ok(())
}

/// Open the user DB standalone (creating it + its schema/builtin tags if needed).
/// Used by the tag-collection server fns that don't touch the dictionary.
#[cfg(feature = "ssr")]
fn open_user() -> Result<rusqlite::Connection, ServerFnError> {
    let u = rusqlite::Connection::open(user_db_path()).map_err(|e| ServerFnError::new(e.to_string()))?;
    u.execute_batch(USER_SCHEMA).map_err(|e| ServerFnError::new(e.to_string()))?;
    migrate_user(&u)?;
    Ok(u)
}

/// Open the dictionary DB with the user DB ATTACHed as `u`, so a single
/// connection can join words/candidates against the user's tags.
#[cfg(feature = "ssr")]
fn open_conn() -> Result<rusqlite::Connection, ServerFnError> {
    open_user()?; // ensure the user DB + schema exist before attaching
    let conn = rusqlite::Connection::open(db_path()).map_err(|e| ServerFnError::new(e.to_string()))?;
    conn.execute("ATTACH DATABASE ?1 AS u", rusqlite::params![user_db_path()])
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(conn)
}

// ---- book import: file staging + Python orchestration (ingest/import_book.py) ----

/// The repo root (parent of the `ui/` crate the server runs from), so Python is
/// invoked with the same cwd its package imports expect. Mirrors db_path()'s
/// "../ vs ." probing.
#[cfg(feature = "ssr")]
pub(crate) fn repo_root() -> String {
    for p in ["..", "."] {
        if std::path::Path::new(p).join("ingest").is_dir() {
            return p.to_string();
        }
    }
    "..".to_string()
}

/// Where dropped books are copied. Precedence: COOLWORDS_BOOKS_DIR env > repo
/// `.env` (same file ingest/paths.py reads) > default. Kept in sync with Python.
#[cfg(feature = "ssr")]
pub(crate) fn books_dir() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("COOLWORDS_BOOKS_DIR") {
        if !p.trim().is_empty() {
            return p.into();
        }
    }
    for envp in ["../.env", ".env"] {
        if let Ok(txt) = std::fs::read_to_string(envp) {
            for line in txt.lines() {
                let line = line.trim();
                if line.starts_with('#') {
                    continue;
                }
                if let Some((k, v)) = line.split_once('=') {
                    if k.trim() == "COOLWORDS_BOOKS_DIR" {
                        let v = v.trim().trim_matches(|c| c == '"' || c == '\'');
                        if !v.is_empty() {
                            return v.into();
                        }
                    }
                }
            }
        }
    }
    r"D:\datasets\coolwords\books".into()
}

/// Staging area for uploaded-but-not-committed files.
#[cfg(feature = "ssr")]
fn staging_dir() -> std::path::PathBuf {
    let d = books_dir().join(".staging");
    let _ = std::fs::create_dir_all(&d);
    d
}

#[cfg(feature = "ssr")]
pub(crate) fn python_exe() -> String {
    std::env::var("COOLWORDS_PYTHON").unwrap_or_else(|_| "python".to_string())
}

/// Run `python -m ingest.import_book <args>` and return its parsed JSON, turning a
/// non-zero exit or an `{"ok": false, ...}` payload into a user-facing error.
#[cfg(feature = "ssr")]
fn run_importer(args: &[&str]) -> Result<serde_json::Value, ServerFnError> {
    let out = std::process::Command::new(python_exe())
        .current_dir(repo_root())
        .arg("-m")
        .arg("ingest.import_book")
        .args(args)
        .output()
        .map_err(|e| ServerFnError::new(format!("could not run python: {e}")))?;
    if !out.status.success() {
        return Err(ServerFnError::new(format!(
            "importer failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| {
        ServerFnError::new(format!(
            "bad JSON from importer: {e} — {}",
            String::from_utf8_lossy(&out.stdout).trim()
        ))
    })?;
    if v.get("ok").and_then(|b| b.as_bool()) != Some(true) {
        let msg = v.get("error").and_then(|s| s.as_str()).unwrap_or("import failed");
        return Err(ServerFnError::new(msg.to_string()));
    }
    Ok(v)
}

/// Canonical book slug: lowercase, ascii-alnum + single dashes, <= 60 chars.
#[cfg(feature = "ssr")]
fn sanitize_slug(s: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in s.trim().to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    out.trim_matches('-').chars().take(60).collect::<String>().trim_matches('-').to_string()
}

/// Tags for a book as word_id -> tags, resolved from the user DB's text keys
/// (book slug + headword) back to dictionary word_ids. Requires `u` attached.
#[cfg(feature = "ssr")]
fn load_tags(
    conn: &rusqlite::Connection,
    book_id: i64,
) -> Result<HashMap<i64, Vec<(String, i32)>>, ServerFnError> {
    use rusqlite::OptionalExtension;
    let slug: Option<String> = conn
        .query_row("SELECT slug FROM books WHERE id = ?1", [book_id], |r| r.get(0))
        .optional()
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let Some(slug) = slug else { return Ok(HashMap::new()) };
    // book-scoped tags for THIS book, plus word-scoped (global, book_slug='*') tags
    // for any of the book's words. `value` is the tri-state rating (NULL==1==applied).
    let mut stmt = conn
        .prepare(
            "SELECT w.id, t.tag, COALESCE(t.value, 1) FROM u.word_tags t JOIN words w ON w.word = t.word
             WHERE (t.book_slug = ?1 OR t.book_slug = '*') AND t.rater = 'me'",
        )
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let mut map: HashMap<i64, Vec<(String, i32)>> = HashMap::new();
    for r in stmt
        .query_map([slug], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i32>(2)?)))
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .filter_map(Result::ok)
    {
        map.entry(r.0).or_default().push((r.1, r.2));
    }
    Ok(map)
}

/// Per-word "pick:" buckets: POS (noun/verb/adj/adv) + full WordNet lexname
/// categories (noun.animal, verb.communication, ...), minus the generic *.all.
#[cfg(feature = "ssr")]
fn load_buckets(conn: &rusqlite::Connection, book_id: i64, level: i64) -> Result<HashMap<i64, Vec<String>>, ServerFnError> {
    let mut m: HashMap<i64, BTreeSet<String>> = HashMap::new();
    let mut s1 = conn
        .prepare(
            "SELECT DISTINCT wp.word_id, wp.pos FROM word_pos wp
             JOIN candidates c ON c.word_id = wp.word_id
             WHERE c.book_id = ?1 AND c.level = ?2 AND wp.pos IN ('noun','verb','adj','adv')",
        )
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    for r in s1.query_map([book_id, level], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| ServerFnError::new(e.to_string()))?.filter_map(Result::ok)
    {
        m.entry(r.0).or_default().insert(r.1);
    }
    let mut s2 = conn
        .prepare(
            "SELECT DISTINCT wc.word_id, wc.category FROM word_category wc
             JOIN candidates c ON c.word_id = wc.word_id
             WHERE c.book_id = ?1 AND c.level = ?2 AND wc.category NOT LIKE '%.all' AND wc.category <> 'noun.Tops'",
        )
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    for r in s2.query_map([book_id, level], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| ServerFnError::new(e.to_string()))?.filter_map(Result::ok)
    {
        m.entry(r.0).or_default().insert(r.1);
    }
    Ok(m.into_iter().map(|(k, v)| (k, v.into_iter().collect())).collect())
}

#[cfg(feature = "ssr")]
fn word_buckets(conn: &rusqlite::Connection, word_id: i64) -> Result<Vec<String>, ServerFnError> {
    let mut set: BTreeSet<String> = BTreeSet::new();
    let mut s1 = conn
        .prepare("SELECT DISTINCT pos FROM word_pos WHERE word_id = ?1 AND pos IN ('noun','verb','adj','adv')")
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    for p in s1.query_map([word_id], |r| r.get::<_, String>(0))
        .map_err(|e| ServerFnError::new(e.to_string()))?.filter_map(Result::ok)
    {
        set.insert(p);
    }
    let mut s2 = conn
        .prepare("SELECT DISTINCT category FROM word_category WHERE word_id = ?1 AND category NOT LIKE '%.all' AND category <> 'noun.Tops'")
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    for c in s2.query_map([word_id], |r| r.get::<_, String>(0))
        .map_err(|e| ServerFnError::new(e.to_string()))?.filter_map(Result::ok)
    {
        set.insert(c);
    }
    Ok(set.into_iter().collect())
}

/// Etymology-language codes that are the *ordinary* substrate of English: Old/
/// Middle English, Latin, the French/Norman family, Ancient Greek, the Germanic/
/// Norse/Dutch/German continuum, plus the generic PIE / translingual roots. The
/// "uncommon" loanword filter keeps words whose etymology language is NOT in this
/// set (e.g. Hindi, Italian, Spanish, Powhatan, Quechua, Persian, Malay…).
#[cfg(feature = "ssr")]
const COMMON_ORIGINS: &[&str] = &[
    "ang", "enm", "enm-nor", "ang-nrt", "la", "la-med", "la-new", "la-vul", "fr", "fro",
    "fro-nor", "frm", "xno", "nrf", "grc", "el", "gem-pro", "gem", "gmw", "gmq", "non",
    "gml", "gmh", "goh", "nds", "osx", "got", "dum", "odt", "nl", "vls", "de", "ine-pro",
    "mul", "sco",
];

/// Romance languages excluded by the *stricter* loanword filter (they descend
/// straight from Latin, so they're barely more exotic than the substrate).
/// Romanian ('ro') is deliberately KEPT — it yields genuinely unusual words
/// (mămăligă, cobza), so it survives even the strict filter.
#[cfg(feature = "ssr")]
const ROMANCE_ORIGINS: &[&str] = &[
    "it", "es", "pt", "ca", "gl", "oc", "an", "co", "rm", "sc", "fur", "lld",
    "nap", "scn", "vec", "lij", "pms", "lmo", "frp", "wa", "mwl", "ext", "lad",
];

/// Quoted SQL `IN`-list of etymology codes to EXCLUDE for the loanword filter.
/// `strict` adds the Romance languages on top of the common substrate.
#[cfg(feature = "ssr")]
fn origin_exclude_sql(strict: bool) -> String {
    // All literals are hardcoded ASCII codes, so direct interpolation is safe.
    COMMON_ORIGINS
        .iter()
        .chain(if strict { ROMANCE_ORIGINS } else { &[] }.iter())
        .map(|c| format!("'{c}'"))
        .collect::<Vec<_>>()
        .join(",")
}

/// Window-average usage (per million) at or below this counts as "rare".
#[cfg(feature = "ssr")]
const RARE_PM: f64 = 0.10;
/// Decades from here onward stand in for "the present day" (the corpus ends in
/// the 2000s decade), used by the obsolete-today test.
#[cfg(feature = "ssr")]
const RECENT_FROM: i32 = 1980;

/// Last decade the usage corpus actually covers (the Google 1-grams end in the
/// 2000s). Used to clamp the era classifier so books published *after* the data
/// aren't misjudged.
#[cfg(feature = "ssr")]
fn corpus_last_decade(conn: &rusqlite::Connection) -> i32 {
    conn.query_row("SELECT COALESCE(MAX(decade), 2000) FROM word_trajectory", [], |r| {
        r.get::<_, i64>(0)
    })
    .map(|d| d as i32)
    .unwrap_or(2000)
}

/// Classify a word's usage trajectory *relative to a book's publication decade* —
/// a fixed historical question, independent of the present day. Returns a key:
/// "ahead" (rare then, common later), "rising" (still climbing at the latest data,
/// for books at/after the corpus end where "ahead" can't be judged), "of" (peaked
/// around the book era), "after" (its heyday was earlier — fading by the book's
/// time), "timeless" (roughly steady), or "rare" (never common). `None` w/o data.
///
/// `corpus_end` is the last decade the data covers. A book published after it
/// (a modern import) has no usage data at its own era, so we'd otherwise read the
/// empty window as zero usage and mislabel rising/steady words as "declining" —
/// clamp the book's decade to `corpus_end` and judge it as of the latest data.
/// (A word that genuinely faded *within* the corpus still has a real near-zero
/// `at` window and correctly stays "after"/declining.)
#[cfg(feature = "ssr")]
fn classify_era(traj: &[(i32, f64)], book_year: Option<i64>, corpus_end: i32) -> Option<&'static str> {
    let year = book_year?;
    if traj.len() < 3 {
        return None;
    }
    let peak = traj.iter().map(|(_, p)| *p).fold(0.0_f64, f64::max);
    if peak < RARE_PM {
        return Some("rare"); // never common in any decade
    }
    let bdec_raw = (year as f64 / 10.0).floor() as i32 * 10;
    // A book at/after the corpus end has no "after" window, so "ahead of its time"
    // (rare-then-common-later) can't be judged. If such a word is still climbing at
    // the latest data, that's "on the rise" — the honest modern-book read.
    if bdec_raw >= corpus_end {
        let mut ds: Vec<(i32, f64)> = traj.to_vec();
        ds.sort_by(|a, b| a.0.cmp(&b.0));
        let last = ds[ds.len() - 1].1;
        let prev = ds[ds.len() - 2].1;
        if last > prev * 1.15 && last >= peak * 0.90 {
            return Some("rising");
        }
    }
    let bdec = bdec_raw.min(corpus_end);
    let mean = |lo: i32, hi: i32| -> Option<f64> {
        let v: Vec<f64> = traj.iter().filter(|(d, _)| *d >= lo && *d <= hi).map(|(_, p)| *p).collect();
        (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64)
    };
    // usage around the book era; missing pre/post windows default to `at` so a
    // book at the edge of the data isn't misread as a trend.
    let at = mean(bdec - 10, bdec + 10).unwrap_or(0.0);
    let before = mean(i32::MIN, bdec - 20).unwrap_or(at);
    let after = mean(bdec + 20, i32::MAX).unwrap_or(at);
    let hi = before.max(at).max(after);
    let lo = before.min(at).min(after);
    if hi <= 0.0 || (hi - lo) / hi < 0.25 {
        return Some("timeless");
    }
    if at >= before && at >= after {
        return Some("of");
    }
    if after > before {
        return Some("ahead");
    }
    Some("after")
}

/// Present-day status (book-independent, orthogonal to `classify_era`): a word
/// that had genuine usage once but is effectively extinct in the most recent
/// decades. Anchored to an absolute recent window so a word that died long ago
/// is still caught (its trajectory simply has no recent points → recent ≈ 0).
#[cfg(feature = "ssr")]
fn is_obsolete_now(traj: &[(i32, f64)]) -> bool {
    if traj.len() < 3 {
        return false;
    }
    let peak = traj.iter().map(|(_, p)| *p).fold(0.0_f64, f64::max);
    if peak < RARE_PM {
        return false; // never common enough to "become" obsolete — that's "always rare"
    }
    let recent = traj.iter().filter(|(d, _)| *d >= RECENT_FROM).map(|(_, p)| *p).fold(0.0_f64, f64::max);
    recent < RARE_PM && recent < 0.15 * peak
}

#[cfg(feature = "ssr")]
fn era_label(key: &str) -> &'static str {
    match key {
        "ahead" => "ahead of its time",
        "rising" => "on the rise",
        "of" => "of its time",
        "after" => "declining",
        "timeless" => "timeless",
        "rare" => "always rare",
        _ => "",
    }
}

/// Trajectories of a book's candidate representatives at one level, keyed by word_id.
#[cfg(feature = "ssr")]
fn book_trajectories(
    conn: &rusqlite::Connection,
    book_id: i64,
    level: i64,
) -> Result<HashMap<i64, Vec<(i32, f64)>>, ServerFnError> {
    let mut m: HashMap<i64, Vec<(i32, f64)>> = HashMap::new();
    let mut s = conn
        .prepare(
            "SELECT t.word_id, t.decade, t.pm FROM word_trajectory t
             JOIN candidates c ON c.word_id = t.word_id
             WHERE c.book_id = ?1 AND c.level = ?2 ORDER BY t.word_id, t.decade",
        )
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    for r in s
        .query_map([book_id, level], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)? as i32, r.get::<_, f64>(2)?)))
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .filter_map(Result::ok)
    {
        m.entry(r.0).or_default().push((r.1, r.2));
    }
    Ok(m)
}

#[cfg(feature = "ssr")]
fn book_year(conn: &rusqlite::Connection, book_id: i64) -> Result<Option<i64>, ServerFnError> {
    use rusqlite::OptionalExtension;
    Ok(conn
        .query_row("SELECT year FROM books WHERE id = ?1", [book_id], |r| r.get::<_, Option<i64>>(0))
        .optional()
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .flatten())
}

/// Word classes offered in the "part of speech" filter, in display order. POS
/// comes from word_pos (WordNet + the wiktextract fallback, so non-WordNet words
/// are still covered). Content classes first, then other classes; affixes /
/// phrases / proper-noun 'name' / punctuation are intentionally omitted.
#[cfg(feature = "ssr")]
const FILTER_POS: &[&str] = &[
    "noun", "verb", "adj", "adv", "intj", "prep", "pron", "conj", "num", "det", "particle", "contraction",
];

#[cfg(feature = "ssr")]
fn pos_label(pos: &str) -> String {
    match pos {
        "intj" => "interjection",
        "prep" => "preposition",
        "pron" => "pronoun",
        "conj" => "conjunction",
        "num" => "numeral",
        "det" => "determiner",
        other => other, // noun / verb / adj / adv / particle / contraction
    }
    .to_string()
}

#[server]
pub async fn list_books() -> Result<Vec<Book>, ServerFnError> {
    let conn = open_conn()?;
    let mut stmt = conn
        .prepare(
            // ★ count = distinct words favourited in this book, i.e. carrying any
            // applied 'interesting'-class tag — book-scoped to this book, or word-scoped
            // (global) AND actually present in this book. A tag also satisfies an
            // interesting ANCESTOR (child implies parent: `t.tag LIKE g.name||'.%'`).
            // COALESCE(value,1)>=1 excludes 0/considered (and legacy NULL == applied).
            "SELECT b.id, COALESCE(b.title, b.slug),
                    (SELECT count(DISTINCT t.word) FROM u.word_tags t
                       JOIN u.tags g ON (g.name = t.tag OR t.tag LIKE g.name || '.%')
                       LEFT JOIN words w ON w.word = t.word
                       LEFT JOIN book_occurrences bo ON bo.word_id = w.id AND bo.book_id = b.id
                     WHERE g.interest = 'interesting' AND COALESCE(t.value, 1) >= 1
                       AND (t.book_slug = b.slug OR (t.book_slug = '*' AND bo.word_id IS NOT NULL)))
             FROM books b ORDER BY b.id",
        )
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let rows = stmt
        .query_map([], |r| Ok(Book { id: r.get(0)?, title: r.get(1)?, n_selected: r.get(2)? }))
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| ServerFnError::new(e.to_string()))?);
    }
    Ok(out)
}

#[server]
pub async fn list_categories(book_id: i64, level: i64) -> Result<Vec<FilterOpt>, ServerFnError> {
    use rusqlite::Connection;
    let conn = Connection::open(db_path()).map_err(|e| ServerFnError::new(e.to_string()))?;
    let mut out: Vec<FilterOpt> = Vec::new();
    let opt = |value: String, label: String, count: i64, group: &str| FilterOpt {
        value, label, count, group: group.to_string(),
    };

    // --- part of speech (WordNet + wiktextract fallback) ---
    let mut ps = conn
        .prepare(
            "SELECT wp.pos, count(DISTINCT c.word_id) FROM candidates c
             JOIN word_pos wp ON wp.word_id = c.word_id
             WHERE c.book_id = ?1 AND c.level = ?2 GROUP BY wp.pos",
        )
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let pos_counts: HashMap<String, i64> = ps
        .query_map([book_id, level], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .filter_map(Result::ok)
        .collect();
    for &p in FILTER_POS {
        if let Some(&n) = pos_counts.get(p) {
            out.push(opt(format!("pos:{p}"), pos_label(p), n, "part of speech"));
        }
    }

    // --- era (relative to the book's year) + present-day status, both classified
    // from each word's trajectory in a single pass ---
    let year = book_year(&conn, book_id)?;
    let corpus_end = corpus_last_decade(&conn);
    let traj = book_trajectories(&conn, book_id, level)?;
    let mut era_counts: HashMap<&str, i64> = HashMap::new();
    let mut n_obsolete = 0i64;
    for t in traj.values() {
        if let Some(k) = classify_era(t, year, corpus_end) {
            *era_counts.entry(k).or_default() += 1;
        }
        if is_obsolete_now(t) {
            n_obsolete += 1;
        }
    }
    for key in ["ahead", "rising", "of", "after", "timeless", "rare"] {
        if let Some(&n) = era_counts.get(key) {
            out.push(opt(format!("era:{key}"), era_label(key).to_string(), n, "era (vs. book's year)"));
        }
    }
    if n_obsolete > 0 {
        out.push(opt(
            "status:obsolete".to_string(),
            "obsolete today".to_string(),
            n_obsolete,
            "present-day status",
        ));
    }

    // --- loanword origin (two tiers): "uncommon" excludes English's core
    // substrate; "rare" also excludes the Romance languages (kept: Romanian). ---
    for (value, label, strict) in [
        ("origin:uncommon", "uncommon", false),
        ("origin:rare", "rare (non-Romance)", true),
    ] {
        let n: i64 = conn
            .query_row(
                &format!(
                    "SELECT count(DISTINCT c.word_id) FROM candidates c JOIN words w ON w.id = c.word_id
                     WHERE c.book_id = ?1 AND c.level = ?2 AND w.etymology_lang IS NOT NULL
                       AND w.etymology_lang NOT IN ({})",
                    origin_exclude_sql(strict)
                ),
                [book_id, level],
                |r| r.get(0),
            )
            .map_err(|e| ServerFnError::new(e.to_string()))?;
        if n > 0 {
            out.push(opt(value.to_string(), label.to_string(), n, "loanword origin"));
        }
    }

    // --- precise WordNet categories (lexnames) ---
    let mut stmt = conn
        .prepare(
            "SELECT wc.category, count(DISTINCT c.word_id) n
             FROM candidates c JOIN word_category wc ON wc.word_id = c.word_id
             WHERE c.book_id = ?1 AND c.level = ?2 GROUP BY wc.category ORDER BY n DESC, wc.category",
        )
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    for r in stmt
        .query_map([book_id, level], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .filter_map(Result::ok)
    {
        out.push(opt(r.0.clone(), r.0, r.1, "category"));
    }

    Ok(out)
}

#[server]
pub async fn get_candidates(
    book_id: i64,
    category: Option<String>,
    limit: i32,
    level: i64,
) -> Result<Vec<Candidate>, ServerFnError> {
    let conn = open_conn()?;
    let tags = load_tags(&conn, book_id)?;
    let buckets = load_buckets(&conn, book_id, level)?;

    // Parse the filter spec held in the `cat` param. Exactly one filter is ever
    // active (single-select dropdown). `era:`/`status:obsolete` are resolved in
    // Rust from each word's trajectory; the rest become a SQL WHERE fragment
    // (the category value, when present, binds as ?3).
    let cat = category.as_deref().unwrap_or("");
    let era_filter = cat.strip_prefix("era:");
    let obsolete_filter = cat == "status:obsolete";
    let needs_traj = era_filter.is_some() || obsolete_filter;
    let mut where_extra = String::new();
    let mut bind_cat: Option<String> = None; // bound as ?3 when present
    if let Some(p) = cat.strip_prefix("pos:") {
        where_extra =
            " AND EXISTS (SELECT 1 FROM word_pos wp WHERE wp.word_id = w.id AND wp.pos = ?3)".into();
        bind_cat = Some(p.to_string());
    } else if cat == "origin:uncommon" || cat == "origin:rare" {
        where_extra = format!(
            " AND w.etymology_lang IS NOT NULL AND w.etymology_lang NOT IN ({})",
            origin_exclude_sql(cat == "origin:rare")
        );
    } else if !needs_traj && !cat.is_empty() {
        where_extra =
            " AND EXISTS (SELECT 1 FROM word_category wc WHERE wc.word_id = w.id AND wc.category = ?3)"
                .into();
        bind_cat = Some(cat.to_string());
    }

    // trajectory filters need every candidate (to classify), so fetch unbounded
    // and truncate after filtering; otherwise push the limit into SQL.
    let limit_sql = if needs_traj { String::new() } else { format!(" LIMIT {}", limit.max(0)) };
    let sql = format!(
        "SELECT w.id, w.word, c.in_book, c.score, w.gloss, w.etymology_lang, ln.name,
                w.wordnet_category, c.cluster, c.selected, bo.example, c.n_forms
         FROM candidates c
         JOIN words w ON w.id = c.word_id
         LEFT JOIN lang_names ln ON ln.code = w.etymology_lang
         LEFT JOIN book_occurrences bo ON bo.book_id = c.book_id AND bo.word_id = c.word_id
         WHERE c.book_id = ?1 AND c.level = ?2{where_extra}
         ORDER BY c.rank{limit_sql}"
    );

    let mut stmt = conn.prepare(&sql).map_err(|e| ServerFnError::new(e.to_string()))?;
    let map_row = |row: &rusqlite::Row| -> rusqlite::Result<Candidate> {
        Ok(Candidate {
            word_id: row.get(0)?,
            word: row.get(1)?,
            in_book: row.get(2)?,
            score: row.get(3)?,
            gloss: row.get(4)?,
            origin_code: row.get(5)?,
            origin_name: row.get(6)?,
            category: row.get(7)?,
            cluster: row.get(8)?,
            selected: row.get::<_, i64>(9)? != 0,
            example: row.get(10)?,
            tags: Vec::new(),
            buckets: Vec::new(),
            n_forms: row.get(11)?,
            members: Vec::new(),
        })
    };
    let mut out: Vec<Candidate> = match &bind_cat {
        Some(v) => stmt
            .query_map(rusqlite::params![book_id, level, v], map_row)
            .map_err(|e| ServerFnError::new(e.to_string()))?
            .filter_map(Result::ok)
            .collect(),
        None => stmt
            .query_map(rusqlite::params![book_id, level], map_row)
            .map_err(|e| ServerFnError::new(e.to_string()))?
            .filter_map(Result::ok)
            .collect(),
    };

    if needs_traj {
        let year = book_year(&conn, book_id)?;
        let corpus_end = corpus_last_decade(&conn);
        let traj = book_trajectories(&conn, book_id, level)?;
        if let Some(era) = era_filter {
            out.retain(|c| {
                traj.get(&c.word_id).and_then(|t| classify_era(t, year, corpus_end)).is_some_and(|k| k == era)
            });
        } else {
            out.retain(|c| traj.get(&c.word_id).is_some_and(|t| is_obsolete_now(t)));
        }
        out.truncate(limit.max(0) as usize);
    }

    // in-book member word_ids per group representative at this level (level 0:
    // each word maps to itself, so members = [self]).
    let mut members_map: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut ms = conn
        .prepare(
            "SELECT COALESCE(wl.lemma_id, bo.word_id) AS rep, bo.word_id
             FROM book_occurrences bo
             LEFT JOIN word_lemma wl ON wl.word_id = bo.word_id AND wl.level = ?2
             WHERE bo.book_id = ?1 AND bo.word_id IS NOT NULL",
        )
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    for (rep, wid) in ms
        .query_map([book_id, level], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .filter_map(Result::ok)
    {
        members_map.entry(rep).or_default().push(wid);
    }

    for c in &mut out {
        if let Some(t) = tags.get(&c.word_id) {
            // applied names only (value >= 1); the client store separately carries
            // the tri-state values via book_tags.
            c.tags = t.iter().filter(|(_, v)| *v >= 1).map(|(n, _)| n.clone()).collect();
        }
        if let Some(b) = buckets.get(&c.word_id) {
            c.buckets = b.clone();
        }
        c.members = members_map.remove(&c.word_id).unwrap_or_else(|| vec![c.word_id]);
    }
    Ok(out)
}

#[server]
pub async fn word_detail(book_id: i64, word_id: i64, level: i64) -> Result<WordInfo, ServerFnError> {
    use rusqlite::{Connection, OptionalExtension};
    let conn = Connection::open(db_path()).map_err(|e| ServerFnError::new(e.to_string()))?;

    let (word, gloss, origin_code, origin_name, freq_pm, syllables, stem, in_book, example, book_year):
        (String, Option<String>, Option<String>, Option<String>, Option<f64>, Option<i64>,
         Option<String>, i64, Option<String>, Option<i64>) = conn
        .query_row(
            "SELECT w.word, w.gloss, w.etymology_lang, ln.name, w.freq_pm, w.syllables, w.stem,
                    COALESCE(bo.count, 0), bo.example, (SELECT year FROM books WHERE id = ?1)
             FROM words w
             LEFT JOIN lang_names ln ON ln.code = w.etymology_lang
             LEFT JOIN book_occurrences bo ON bo.word_id = w.id AND bo.book_id = ?1
             WHERE w.id = ?2",
            (book_id, word_id),
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?,
                    r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?)),
        )
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let mut catstmt = conn
        .prepare("SELECT category FROM word_category WHERE word_id = ?1 ORDER BY is_primary DESC, category")
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let categories: Vec<String> = catstmt
        .query_map([word_id], |r| r.get::<_, String>(0))
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .filter_map(Result::ok)
        .collect();

    let buckets = word_buckets(&conn, word_id)?;

    let mut base = None;
    if let Some(st) = &stem {
        if st != &word {
            let bf: Option<f64> = conn
                .query_row("SELECT freq_pm FROM words WHERE word = ?1", rusqlite::params![st],
                    |r| r.get::<_, Option<f64>>(0))
                .optional()
                .map_err(|e| ServerFnError::new(e.to_string()))?
                .flatten();
            if let Some(f) = bf {
                if f > freq_pm.unwrap_or(0.0) {
                    base = Some((st.clone(), f));
                }
            }
        }
    }

    // The merged group at this level: the representative plus every in-book
    // surface form whose lemma-at-level resolves to it. (At level 0 there are no
    // word_lemma rows, so this is just the word itself and the section hides.)
    let mut family = Vec::new();
    let mut fstmt = conn
        .prepare(
            "SELECT w.id, w.word, bo.count, w.freq_pm
             FROM book_occurrences bo JOIN words w ON w.id = bo.word_id
             WHERE bo.book_id = ?1 AND w.alpha_only = 1 AND (
                 w.id = ?2
                 OR EXISTS (SELECT 1 FROM word_lemma wl
                            WHERE wl.word_id = w.id AND wl.level = ?3 AND wl.lemma_id = ?2))
             ORDER BY w.freq_pm DESC LIMIT 15",
        )
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let rows = fstmt
        .query_map(rusqlite::params![book_id, word_id, level], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?,
                r.get::<_, Option<f64>>(3)?.unwrap_or(0.0)))
        })
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    for r in rows {
        family.push(r.map_err(|e| ServerFnError::new(e.to_string()))?);
    }

    let mut relstmt = conn
        .prepare(
            "SELECT wr.rel, wr.target, tw.id,
                    CASE WHEN bo.word_id IS NOT NULL THEN 1 ELSE 0 END
             FROM word_relation wr
             LEFT JOIN words tw ON tw.word = wr.target
             LEFT JOIN book_occurrences bo ON bo.word_id = tw.id AND bo.book_id = ?2
             WHERE wr.word_id = ?1
             ORDER BY CASE wr.rel
                 WHEN 'hypernym' THEN 1 WHEN 'coordinate' THEN 2
                 WHEN 'part meronym' THEN 3 WHEN 'member meronym' THEN 4 WHEN 'substance meronym' THEN 5
                 WHEN 'part holonym' THEN 6 WHEN 'member holonym' THEN 7 WHEN 'substance holonym' THEN 8
                 WHEN 'antonym' THEN 9 WHEN 'derivation' THEN 10 WHEN 'pertainym' THEN 11 ELSE 12 END,
                 (bo.word_id IS NOT NULL) DESC, tw.freq_pm DESC, wr.target
             LIMIT 120",
        )
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let relations: Vec<RelTarget> = relstmt
        .query_map(rusqlite::params![word_id, book_id], |r| {
            Ok(RelTarget {
                rel: r.get(0)?,
                target: r.get(1)?,
                target_word_id: r.get(2)?,
                in_book: r.get::<_, i64>(3)? != 0,
            })
        })
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .filter_map(Result::ok)
        .collect();

    let mut tstmt = conn
        .prepare("SELECT decade, pm FROM word_trajectory WHERE word_id = ?1 ORDER BY decade")
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let trajectory: Vec<(i32, f64)> = tstmt
        .query_map([word_id], |r| Ok((r.get::<_, i64>(0)? as i32, r.get::<_, f64>(1)?)))
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .filter_map(Result::ok)
        .collect();

    let era = classify_era(&trajectory, book_year, corpus_last_decade(&conn)).map(|k| era_label(k).to_string());
    let obsolete = is_obsolete_now(&trajectory);

    // Root/lemma (when distinct from the headword): its own usage chart + category,
    // so e.g. "harpooneer" also shows the trajectory of "harpoon".
    let mut root = None;
    if let Some(st) = &stem {
        if st != &word {
            let row = conn
                .query_row(
                    "SELECT id, freq_pm, wordnet_category FROM words WHERE word = ?1",
                    rusqlite::params![st],
                    |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<f64>>(1)?, r.get::<_, Option<String>>(2)?)),
                )
                .optional()
                .map_err(|e| ServerFnError::new(e.to_string()))?;
            if let Some((rid, rfreq, rcat0)) = row {
                let mut rts = conn
                    .prepare("SELECT decade, pm FROM word_trajectory WHERE word_id = ?1 ORDER BY decade")
                    .map_err(|e| ServerFnError::new(e.to_string()))?;
                let rtraj: Vec<(i32, f64)> = rts
                    .query_map([rid], |r| Ok((r.get::<_, i64>(0)? as i32, r.get::<_, f64>(1)?)))
                    .map_err(|e| ServerFnError::new(e.to_string()))?
                    .filter_map(Result::ok)
                    .collect();
                let category = rcat0.or_else(|| {
                    conn.query_row(
                        "SELECT category FROM word_category WHERE word_id = ?1 ORDER BY is_primary DESC, category LIMIT 1",
                        [rid],
                        |r| r.get::<_, String>(0),
                    )
                    .optional()
                    .ok()
                    .flatten()
                });
                // only worth a second chart if the root carries some usage history
                if !rtraj.is_empty() {
                    root = Some(RootInfo { word: st.clone(), word_id: rid, freq_pm: rfreq, category, trajectory: rtraj });
                }
            }
        }
    }

    // Other books containing this word or any level-family member — shows the
    // reach of a word-scoped (global) tag. At level 0 the word_lemma EXISTS is
    // empty, so this is just books containing the headword itself.
    let mut also_in = Vec::new();
    let mut aistmt = conn
        .prepare(
            "SELECT b.id, COALESCE(b.title, b.slug), SUM(bo.count) AS cnt
             FROM book_occurrences bo JOIN books b ON b.id = bo.book_id
             WHERE bo.book_id <> ?1 AND (
                 bo.word_id = ?2
                 OR EXISTS (SELECT 1 FROM word_lemma wl
                            WHERE wl.word_id = bo.word_id AND wl.level = ?3 AND wl.lemma_id = ?2))
             GROUP BY b.id ORDER BY cnt DESC, b.title LIMIT 20",
        )
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    for r in aistmt
        .query_map(rusqlite::params![book_id, word_id, level], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?))
        })
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .filter_map(Result::ok)
    {
        also_in.push(r);
    }

    Ok(WordInfo {
        word_id, word, gloss, origin_code, origin_name, freq_pm, syllables, in_book, example,
        book_year, categories, buckets, base, family, relations, trajectory, era, obsolete, root,
        also_in,
    })
}

/// Resolve the stable text application key for a (book, word, tag): the word's
/// headword and the `book_slug` to store under — the '*' sentinel for word-scoped
/// tags (they apply across every book), the real slug otherwise. None if the book or
/// word id is unknown. Requires `u` attached (open_conn). Shared by set_tag/set_tag_value.
#[cfg(feature = "ssr")]
fn apply_key(
    conn: &rusqlite::Connection,
    book_id: i64,
    word_id: i64,
    tag: &str,
) -> Result<Option<(String, String)>, ServerFnError> {
    use rusqlite::OptionalExtension;
    let slug: Option<String> = conn
        .query_row("SELECT slug FROM books WHERE id = ?1", [book_id], |r| r.get(0))
        .optional()
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let word: Option<String> = conn
        .query_row("SELECT word FROM words WHERE id = ?1", [word_id], |r| r.get(0))
        .optional()
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let (Some(slug), Some(word)) = (slug, word) else {
        return Ok(None);
    };
    let scope: String = if tag.starts_with("pick:") {
        SCOPE_BOOK.to_string()
    } else {
        conn.query_row("SELECT scope FROM u.tags WHERE name = ?1", [tag], |r| r.get(0))
            .optional()
            .map_err(|e| ServerFnError::new(e.to_string()))?
            .unwrap_or_else(|| SCOPE_BOOK.to_string())
    };
    let key_slug = if scope == SCOPE_WORD { "*".to_string() } else { slug };
    Ok(Some((key_slug, word)))
}

/// Auto-create the ancestor tags of a dotted name (child-implies-parent means the
/// parent must exist as a real, editable tag with its own interest — else the
/// hierarchy would manufacture phantom favourites). Parents default to neutral.
/// `tags_table` is "u.tags" (open_conn) or "tags" (open_user).
#[cfg(feature = "ssr")]
fn ensure_parents(
    conn: &rusqlite::Connection,
    tags_table: &str,
    name: &str,
) -> Result<(), ServerFnError> {
    for anc in ancestor_names(name) {
        conn.execute(
            &format!(
                "INSERT OR IGNORE INTO {tags_table}(name, interest, sort, created)
                 VALUES (?1, 'neutral', 100, datetime('now'))"
            ),
            rusqlite::params![anc],
        )
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    }
    Ok(())
}

#[server]
pub async fn set_tag(book_id: i64, word_id: i64, tag: String, on: bool) -> Result<(), ServerFnError> {
    if !tag_allowed(&tag) {
        return Err(ServerFnError::new("invalid tag"));
    }
    let conn = open_conn()?;
    let is_pick = tag.starts_with("pick:");
    // auto-register a brand-new custom tag (+ its ancestors) so its scope exists.
    if on && !is_pick {
        conn.execute(
            "INSERT OR IGNORE INTO u.tags(name, sort, created) VALUES (?1, 100, datetime('now'))",
            rusqlite::params![tag],
        )
        .map_err(|e| ServerFnError::new(e.to_string()))?;
        ensure_parents(&conn, "u.tags", &tag)?;
    }
    let Some((key_slug, word)) = apply_key(&conn, book_id, word_id, &tag)? else {
        return Err(ServerFnError::new("unknown book or word"));
    };
    if on {
        // Upsert value=1 so turning a previously-declined (value 0) tag on actually
        // applies it (a plain INSERT OR IGNORE would leave the 0 in place).
        conn.execute(
            "INSERT INTO u.word_tags(book_slug, word, tag, rater, ts, value)
             VALUES (?1, ?2, ?3, 'me', datetime('now'), 1)
             ON CONFLICT(book_slug, word, tag, rater) DO UPDATE SET value = 1",
            rusqlite::params![key_slug, word, tag],
        )
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    } else {
        conn.execute(
            "DELETE FROM u.word_tags WHERE book_slug = ?1 AND word = ?2 AND tag = ?3 AND rater = 'me'",
            rusqlite::params![key_slug, word, tag],
        )
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    }
    Ok(())
}

/// Set a tag's tri-state value on a word: None removes the row (untagged); Some(0)
/// records "considered & declined" (remembered, not applied); Some(n) applies at
/// level n (clamped to the tag's scale ceiling). The application key follows the
/// tag's scope exactly like set_tag.
#[server]
pub async fn set_tag_value(
    book_id: i64,
    word_id: i64,
    tag: String,
    value: Option<i32>,
) -> Result<(), ServerFnError> {
    use rusqlite::OptionalExtension;
    if !tag_allowed(&tag) {
        return Err(ServerFnError::new("invalid tag"));
    }
    // 0 = considered-declined, >=1 = applied level; a negative value is meaningless.
    if matches!(value, Some(v) if v < 0) {
        return Err(ServerFnError::new("invalid tag value"));
    }
    let conn = open_conn()?;
    let is_pick = tag.starts_with("pick:");
    // A value>=1 or 0 both need the tag (and its ancestors) to exist as a definition.
    if value.is_some() && !is_pick {
        conn.execute(
            "INSERT OR IGNORE INTO u.tags(name, sort, created) VALUES (?1, 100, datetime('now'))",
            rusqlite::params![tag],
        )
        .map_err(|e| ServerFnError::new(e.to_string()))?;
        ensure_parents(&conn, "u.tags", &tag)?;
    }
    // clamp a positive level to this tag's scale ceiling (defensive).
    let value = value.map(|v| {
        if v <= 0 {
            v.max(0)
        } else if is_pick {
            1
        } else {
            let max: i64 = conn
                .query_row("SELECT scale_max FROM u.tags WHERE name = ?1", [&tag], |r| r.get(0))
                .optional()
                .ok()
                .flatten()
                .unwrap_or(1);
            v.min(max.max(1) as i32)
        }
    });
    let Some((key_slug, word)) = apply_key(&conn, book_id, word_id, &tag)? else {
        return Err(ServerFnError::new("unknown book or word"));
    };
    match value {
        Some(v) => {
            conn.execute(
                "INSERT INTO u.word_tags(book_slug, word, tag, rater, ts, value)
                 VALUES (?1, ?2, ?3, 'me', datetime('now'), ?4)
                 ON CONFLICT(book_slug, word, tag, rater) DO UPDATE SET value = ?4",
                rusqlite::params![key_slug, word, tag, v],
            )
            .map_err(|e| ServerFnError::new(e.to_string()))?;
        }
        None => {
            conn.execute(
                "DELETE FROM u.word_tags WHERE book_slug = ?1 AND word = ?2 AND tag = ?3 AND rater = 'me'",
                rusqlite::params![key_slug, word, tag],
            )
            .map_err(|e| ServerFnError::new(e.to_string()))?;
        }
    }
    Ok(())
}

/// All tags for a book, keyed by word_id — used to seed the client store so tag
/// state is known for every surface form (not just the candidate representatives),
/// which makes the family union (cross-level visibility) reactive.
#[server]
pub async fn book_tags(book_id: i64) -> Result<Vec<(i64, Vec<(String, i32)>)>, ServerFnError> {
    let conn = open_conn()?;
    Ok(load_tags(&conn, book_id)?.into_iter().collect())
}

/// Words carrying the user's collection tags, across every book — the data for the
/// Collection page. `filter` is None (all), a tag name (that tag only), or the
/// special key `"special:top-global"` (words with a positive net of favourite −
/// negative all-books tags, ranked by that net). Book-scoped applications resolve
/// to their one book; word-scoped ('*') applications resolve to every book whose
/// text actually contains the word. Aggregated one row per word.
#[server]
pub async fn collection_words(filter: Option<String>) -> Result<Vec<CollectionEntry>, ServerFnError> {
    use rusqlite::OptionalExtension;
    let conn = open_conn()?;
    // The special "top all-books favourites" view ignores any tag filter and needs
    // every application to compute each word's net; a real tag name narrows the rows.
    let special = filter.as_deref() == Some("special:top-global");
    let tag = if special { None } else { filter };
    // slug -> (id, title) for every book, to resolve book-scoped applications.
    let mut books_by_slug: HashMap<String, (i64, String)> = HashMap::new();
    {
        let mut s = conn
            .prepare("SELECT id, slug, COALESCE(title, slug) FROM books")
            .map_err(|e| ServerFnError::new(e.to_string()))?;
        for r in s
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))
            .map_err(|e| ServerFnError::new(e.to_string()))?
            .filter_map(Result::ok)
        {
            books_by_slug.insert(r.1, (r.0, r.2));
        }
    }

    // One accumulator per word.
    struct Acc {
        word_id: i64,
        gloss: Option<String>,
        tags: Vec<(String, String)>,        // (name, interest), deduped
        books: Vec<(i64, String)>,          // deduped by id
        global_tags: Vec<(String, String)>, // word-scoped ('*') tags only, deduped — drives the net metric
    }
    let mut by_word: HashMap<String, Acc> = HashMap::new();
    // Cache the book list for each word-scoped word (avoids repeat queries).
    let mut word_books: HashMap<String, Vec<(i64, String)>> = HashMap::new();

    // Every APPLIED collection-tag application (the JOIN drops `pick:` buckets, which
    // aren't registered in u.tags; COALESCE(value,1)>=1 drops 0/considered rows).
    // Filter to one tag when requested — a parent name also matches its descendants
    // (`t.tag LIKE ?1 || '.%'`), so filtering by `thing` surfaces `thing.material`.
    let base_sql = "SELECT t.word, t.tag, g.interest, t.book_slug \
                    FROM u.word_tags t JOIN u.tags g ON g.name = t.tag \
                    WHERE t.rater = 'me' AND COALESCE(t.value, 1) >= 1";
    let rows: Vec<(String, String, String, String)> = if let Some(tg) = tag.as_deref() {
        let mut s = conn
            .prepare(&format!("{base_sql} AND (t.tag = ?1 OR t.tag LIKE ?1 || '.%')"))
            .map_err(|e| ServerFnError::new(e.to_string()))?;
        let v = s
            .query_map([tg], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .map_err(|e| ServerFnError::new(e.to_string()))?
            .filter_map(Result::ok)
            .collect();
        v
    } else {
        let mut s = conn.prepare(base_sql).map_err(|e| ServerFnError::new(e.to_string()))?;
        let v = s
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .map_err(|e| ServerFnError::new(e.to_string()))?
            .filter_map(Result::ok)
            .collect();
        v
    };

    for (word, tagname, interest, book_slug) in rows {
        // Resolve this application's books.
        let app_books: Vec<(i64, String)> = if book_slug == "*" {
            if let Some(v) = word_books.get(&word) {
                v.clone()
            } else {
                let mut s = conn
                    .prepare(
                        "SELECT b.id, COALESCE(b.title, b.slug) FROM books b \
                         JOIN book_occurrences bo ON bo.book_id = b.id \
                         JOIN words w ON w.id = bo.word_id WHERE w.word = ?1 ORDER BY b.id",
                    )
                    .map_err(|e| ServerFnError::new(e.to_string()))?;
                let v: Vec<(i64, String)> = s
                    .query_map([&word], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
                    .map_err(|e| ServerFnError::new(e.to_string()))?
                    .filter_map(Result::ok)
                    .collect();
                word_books.insert(word.clone(), v.clone());
                v
            }
        } else {
            books_by_slug.get(&book_slug).cloned().into_iter().collect()
        };

        if !by_word.contains_key(&word) {
            let (wid, gloss): (i64, Option<String>) = conn
                .query_row("SELECT id, gloss FROM words WHERE word = ?1", [&word], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .optional()
                .map_err(|e| ServerFnError::new(e.to_string()))?
                .unwrap_or((0, None));
            by_word.insert(word.clone(), Acc {
                word_id: wid, gloss, tags: Vec::new(), books: Vec::new(), global_tags: Vec::new(),
            });
        }
        let acc = by_word.get_mut(&word).unwrap();
        if book_slug == "*" && !acc.global_tags.iter().any(|(n, _)| n == &tagname) {
            acc.global_tags.push((tagname.clone(), interest.clone()));
        }
        if !acc.tags.iter().any(|(n, _)| n == &tagname) {
            acc.tags.push((tagname, interest));
        }
        for b in app_books {
            if !acc.books.iter().any(|(id, _)| *id == b.0) {
                acc.books.push(b);
            }
        }
    }

    // tag name -> interest, so a word's effective interest can fold in the interest of
    // any ANCESTOR of an applied tag (child implies parent) — matching the words page /
    // book ★-count, instead of contradicting them.
    let mut tint: HashMap<String, String> = HashMap::new();
    {
        let mut s = conn
            .prepare("SELECT name, interest FROM u.tags")
            .map_err(|e| ServerFnError::new(e.to_string()))?;
        for r in s
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| ServerFnError::new(e.to_string()))?
            .filter_map(Result::ok)
        {
            tint.insert(r.0, r.1);
        }
    }
    // Effective interest for sort/section: interesting > uninteresting > neutral, over
    // each applied tag AND its existing ancestors.
    let eff = |tags: &[(String, String)]| -> String {
        let mut interesting = false;
        let mut uninteresting = false;
        for (name, i) in tags {
            let mut consider = |iv: &str| match iv {
                "interesting" => interesting = true,
                "uninteresting" => uninteresting = true,
                _ => {}
            };
            consider(i);
            for anc in ancestor_names(name) {
                if let Some(iv) = tint.get(&anc) {
                    consider(iv);
                }
            }
        }
        if interesting { "interesting".to_string() }
        else if uninteresting { "uninteresting".to_string() }
        else { "neutral".to_string() }
    };
    let int_rank = |i: &str| match i { "interesting" => 0, "neutral" => 1, _ => 2 };
    let net = |gt: &[(String, String)]| -> i64 {
        gt.iter().filter(|(_, i)| i == "interesting").count() as i64
            - gt.iter().filter(|(_, i)| i == "uninteresting").count() as i64
    };
    let mut out: Vec<CollectionEntry> = by_word
        .into_iter()
        .map(|(word, a)| {
            let mut tags = a.tags;
            tags.sort_by(|x, y| int_rank(&x.1).cmp(&int_rank(&y.1)).then_with(|| x.0.cmp(&y.0)));
            let metric = special.then(|| net(&a.global_tags));
            CollectionEntry { interest: eff(&tags), word, word_id: a.word_id, gloss: a.gloss, tags, books: a.books, metric }
        })
        .collect();
    if special {
        // keep only words with a positive net of all-books favourites; rank by it.
        out.retain(|e| e.metric.unwrap_or(0) > 0);
        out.sort_by(|x, y| y.metric.cmp(&x.metric).then_with(|| x.word.cmp(&y.word)));
    } else {
        out.sort_by(|x, y| int_rank(&x.interest).cmp(&int_rank(&y.interest)).then_with(|| x.word.cmp(&y.word)));
    }
    Ok(out)
}

/// The user's collection tags with how many distinct words each is applied to —
/// the counts shown in the Collection page's filter dropdown. Ordered like the
/// tag collection (sort, name). Includes tags with a zero count.
#[server]
pub async fn collection_tags() -> Result<Vec<(String, i64)>, ServerFnError> {
    let conn = open_user()?;
    let mut s = conn
        .prepare(
            // A tag's count includes its descendants' applications (child implies
            // parent), matching the filtered collection view; 0/considered excluded.
            "SELECT g.name, count(DISTINCT t.word) FROM tags g \
             LEFT JOIN word_tags t ON (t.tag = g.name OR t.tag LIKE g.name || '.%') \
                 AND t.rater = 'me' AND COALESCE(t.value, 1) >= 1 \
             GROUP BY g.name ORDER BY g.sort, g.name",
        )
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let out = s
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .filter_map(Result::ok)
        .collect();
    Ok(out)
}

/// The user's whole tag collection (builtin defaults + custom), for the picker.
#[server]
pub async fn list_tags() -> Result<Vec<TagDef>, ServerFnError> {
    let conn = open_user()?;
    let mut stmt = conn
        .prepare("SELECT name, comment, builtin, scope, interest, sort, section, kind, scale_max, scale_labels FROM tags ORDER BY sort, name")
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let out = stmt
        .query_map([], |r| {
            Ok(TagDef {
                name: r.get(0)?,
                comment: r.get(1)?,
                builtin: r.get::<_, i64>(2)? != 0,
                scope: r.get(3)?,
                interest: r.get(4)?,
                sort: r.get(5)?,
                section: r.get(6)?,
                kind: r.get(7)?,
                scale_max: r.get(8)?,
                scale_labels: r.get(9)?,
            })
        })
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .filter_map(Result::ok)
        .collect();
    Ok(out)
}

/// Add a custom tag to the collection (or update its comment if it exists).
/// Returns the canonical tag name so the client can apply it.
#[server]
pub async fn add_tag(name: String, comment: String) -> Result<String, ServerFnError> {
    let clean = sanitize_tag(&name).ok_or_else(|| ServerFnError::new("invalid tag name"))?;
    let comment = comment.trim();
    let comment_opt: Option<&str> = (!comment.is_empty()).then_some(comment);
    let conn = open_user()?;
    conn.execute(
        "INSERT INTO tags(name, comment, builtin, sort, created) VALUES (?1, ?2, 0, 100, datetime('now'))
         ON CONFLICT(name) DO UPDATE SET comment = COALESCE(excluded.comment, tags.comment)",
        rusqlite::params![clean, comment_opt],
    )
    .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(clean)
}

/// Create a tag with an explicit scope + interest (the picker/manager adders).
/// scope/interest apply only to a *brand-new* tag; re-adding an existing name only
/// refreshes its comment. If the (resulting) tag is word-scoped, its applications
/// are collapsed to the global '*' sentinel so create-then-apply is order-safe.
/// Validate a tag `kind` + `scale_max`: 'scale' clamps the ceiling to 2..=10;
/// anything else is a plain 'bool' with ceiling 1.
#[cfg(feature = "ssr")]
fn norm_kind(kind: &str, scale_max: i64) -> (&'static str, i64) {
    if kind == "scale" {
        ("scale", scale_max.clamp(2, 10))
    } else {
        ("bool", 1)
    }
}

#[server]
pub async fn create_tag(
    name: String,
    comment: String,
    scope: String,
    interest: String,
    kind: String,
    scale_max: i64,
) -> Result<String, ServerFnError> {
    use rusqlite::OptionalExtension;
    let clean = sanitize_tag(&name).ok_or_else(|| ServerFnError::new("invalid tag name"))?;
    if !valid_scope(&scope) {
        return Err(ServerFnError::new("invalid scope"));
    }
    if !valid_interest(&interest) {
        return Err(ServerFnError::new("invalid interest level"));
    }
    let (kind, scale_max) = norm_kind(&kind, scale_max);
    let comment = comment.trim();
    let comment_opt: Option<&str> = (!comment.is_empty()).then_some(comment);
    let conn = open_user()?;
    conn.execute(
        "INSERT INTO tags(name, comment, builtin, sort, created, scope, interest, kind, scale_max)
         VALUES (?1, ?2, 0, 100, datetime('now'), ?3, ?4, ?5, ?6)
         ON CONFLICT(name) DO UPDATE SET comment = COALESCE(excluded.comment, tags.comment)",
        rusqlite::params![clean, comment_opt, scope, interest, kind, scale_max],
    )
    .map_err(|e| ServerFnError::new(e.to_string()))?;
    ensure_parents(&conn, "tags", &clean)?;
    // If this tag ends up word-scoped, collapse any book-keyed applications into the
    // global '*' sentinel (mirrors set_tag_scope's book→word migration) so a racing
    // set_tag that auto-registered it book-scoped can't leave the rows mis-keyed.
    // `value` is carried so scale ratings / 0-considered rows survive the re-home.
    let actual: String = conn
        .query_row("SELECT scope FROM tags WHERE name = ?1", [&clean], |r| r.get(0))
        .optional()
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .unwrap_or_else(|| SCOPE_BOOK.to_string());
    if actual == SCOPE_WORD {
        conn.execute(
            // Several book-scoped rows for the same word collapse to one '*' row;
            // applied-wins (MAX of COALESCE(value,1)) so a rating in one book isn't
            // clobbered by a 0/considered in another. GROUP BY makes the pick deterministic.
            "INSERT OR IGNORE INTO word_tags(book_slug, word, tag, rater, ts, value)
             SELECT '*', word, tag, rater, MAX(ts), MAX(COALESCE(value, 1))
             FROM word_tags WHERE tag = ?1 AND book_slug <> '*'
             GROUP BY word, tag, rater",
            [&clean],
        )
        .map_err(|e| ServerFnError::new(e.to_string()))?;
        conn.execute("DELETE FROM word_tags WHERE tag = ?1 AND book_slug <> '*'", [&clean])
            .map_err(|e| ServerFnError::new(e.to_string()))?;
    }
    Ok(clean)
}

/// Atomically create a tag (with the chosen scope + interest) AND apply it to a
/// word, in a single connection — the picker's "add" path. Doing both here removes
/// the create/`set_tag` race that let a concurrent auto-register (which defaults
/// scope='book'/interest='interesting') land first and leave `create_tag`'s
/// ON CONFLICT unable to set them. Applying uses the same key rule as `set_tag`
/// (word scope → the global '*' sentinel).
#[server]
pub async fn create_and_apply_tag(
    name: String,
    comment: String,
    scope: String,
    interest: String,
    kind: String,
    scale_max: i64,
    book_id: i64,
    word_id: i64,
) -> Result<String, ServerFnError> {
    use rusqlite::OptionalExtension;
    let clean = sanitize_tag(&name).ok_or_else(|| ServerFnError::new("invalid tag name"))?;
    if !valid_scope(&scope) {
        return Err(ServerFnError::new("invalid scope"));
    }
    if !valid_interest(&interest) {
        return Err(ServerFnError::new("invalid interest level"));
    }
    let (kind, scale_max) = norm_kind(&kind, scale_max);
    let comment = comment.trim();
    let comment_opt: Option<&str> = (!comment.is_empty()).then_some(comment);
    let conn = open_conn()?;
    // resolve the stable text keys for this (book, word)
    let slug: Option<String> = conn
        .query_row("SELECT slug FROM books WHERE id = ?1", [book_id], |r| r.get(0))
        .optional()
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let word: Option<String> = conn
        .query_row("SELECT word FROM words WHERE id = ?1", [word_id], |r| r.get(0))
        .optional()
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let (Some(slug), Some(word)) = (slug, word) else {
        return Err(ServerFnError::new("unknown book or word"));
    };
    // create (or refresh the comment of) the tag; a brand-new tag takes the chosen
    // scope + interest + kind. Re-adding an existing name only refreshes its comment —
    // the client only calls this for genuinely new names, so nothing clobbers.
    conn.execute(
        "INSERT INTO u.tags(name, comment, builtin, sort, created, scope, interest, kind, scale_max)
         VALUES (?1, ?2, 0, 100, datetime('now'), ?3, ?4, ?5, ?6)
         ON CONFLICT(name) DO UPDATE SET comment = COALESCE(excluded.comment, tags.comment)",
        rusqlite::params![clean, comment_opt, scope, interest, kind, scale_max],
    )
    .map_err(|e| ServerFnError::new(e.to_string()))?;
    ensure_parents(&conn, "u.tags", &clean)?;
    // key the application by the tag's actual (post-conflict) scope.
    let actual: String = conn
        .query_row("SELECT scope FROM u.tags WHERE name = ?1", [&clean], |r| r.get(0))
        .optional()
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .unwrap_or_else(|| SCOPE_BOOK.to_string());
    let key_slug = if actual == SCOPE_WORD { "*".to_string() } else { slug };
    conn.execute(
        "INSERT INTO u.word_tags(book_slug, word, tag, rater, ts, value)
         VALUES (?1, ?2, ?3, 'me', datetime('now'), 1)
         ON CONFLICT(book_slug, word, tag, rater) DO UPDATE SET value = 1",
        rusqlite::params![key_slug, word, clean],
    )
    .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(clean)
}

/// Total applications of a tag across every book key — used to warn before a scope
/// change, which re-homes or drops applications and generally can't be undone.
#[server]
pub async fn tag_usage(name: String) -> Result<i64, ServerFnError> {
    let conn = open_user()?;
    // count APPLIED rows only (value >= 1, my ratings) — a 0/considered row isn't an
    // application, so it shouldn't inflate the scope-change warning's count.
    conn.query_row(
        "SELECT count(*) FROM word_tags WHERE tag = ?1 AND rater = 'me' AND COALESCE(value, 1) >= 1",
        [&name],
        |r| r.get(0),
    )
    .map_err(|e| ServerFnError::new(e.to_string()))
}

/// Rename a tag, cascading to all its applications AND its dotted descendants
/// (`thing` → `stuff` also moves `thing.material` → `stuff.material`). `star` is locked.
#[server]
pub async fn rename_tag(old: String, new: String) -> Result<String, ServerFnError> {
    use rusqlite::OptionalExtension;
    if old == "star" {
        return Err(ServerFnError::new("the ★ tag can't be renamed"));
    }
    // `old` must be a canonical name — it's interpolated into a LIKE subtree pattern
    // below, and this is a public endpoint, so reject anything that isn't already clean
    // (blocks `%`/`_` wildcard injection into the cascade).
    if sanitize_tag(&old).as_deref() != Some(old.as_str()) {
        return Err(ServerFnError::new("invalid tag name"));
    }
    let clean = sanitize_tag(&new).ok_or_else(|| ServerFnError::new("invalid tag name"))?;
    if clean == old {
        return Ok(clean);
    }
    // Renaming a tag into its own subtree (thing → thing.x) would be circular.
    if is_ancestor(&old, &clean) {
        return Err(ServerFnError::new("can't rename a tag under itself"));
    }
    let mut conn = open_user()?;
    // Collision if the new name — or anything in its would-be subtree — already exists.
    let exists = conn
        .query_row(
            "SELECT 1 FROM tags WHERE name = ?1 OR name LIKE ?1 || '.%' LIMIT 1",
            [&clean],
            |_| Ok(()),
        )
        .optional()
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .is_some();
    if exists {
        return Err(ServerFnError::new("a tag with that name already exists"));
    }
    let tx = conn.transaction().map_err(|e| ServerFnError::new(e.to_string()))?;
    // The tag itself, then its descendants (re-prefixing the part after `old`).
    tx.execute("UPDATE tags SET name = ?1 WHERE name = ?2", rusqlite::params![clean, old])
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    tx.execute(
        "UPDATE tags SET name = ?1 || substr(name, ?3) WHERE name LIKE ?2 || '.%'",
        rusqlite::params![clean, old, old.len() as i64 + 1],
    )
    .map_err(|e| ServerFnError::new(e.to_string()))?;
    tx.execute("UPDATE word_tags SET tag = ?1 WHERE tag = ?2", rusqlite::params![clean, old])
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    tx.execute(
        "UPDATE word_tags SET tag = ?1 || substr(tag, ?3) WHERE tag LIKE ?2 || '.%'",
        rusqlite::params![clean, old, old.len() as i64 + 1],
    )
    .map_err(|e| ServerFnError::new(e.to_string()))?;
    // The new name may itself be dotted (foo → beta.gamma); keep the "every ancestor
    // exists as a real tag" invariant that the create paths maintain.
    ensure_parents(&tx, "tags", &clean)?;
    tx.commit().map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(clean)
}

/// Delete a tag, its dotted descendants, and all their applications; returns how
/// many applications were removed (the whole subtree).
#[server]
pub async fn delete_tag(name: String) -> Result<i64, ServerFnError> {
    if name == "star" {
        return Err(ServerFnError::new("the ★ tag can't be deleted"));
    }
    // `name` is interpolated into a LIKE subtree pattern; reject anything non-canonical
    // (public endpoint — blocks `%`/`_` wildcard injection into the cascade).
    if sanitize_tag(&name).as_deref() != Some(name.as_str()) {
        return Err(ServerFnError::new("invalid tag name"));
    }
    let mut conn = open_user()?;
    let tx = conn.transaction().map_err(|e| ServerFnError::new(e.to_string()))?;
    let n = tx
        .execute("DELETE FROM word_tags WHERE tag = ?1 OR tag LIKE ?1 || '.%'", [&name])
        .map_err(|e| ServerFnError::new(e.to_string()))? as i64;
    tx.execute("DELETE FROM tags WHERE name = ?1 OR name LIKE ?1 || '.%'", [&name])
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    tx.commit().map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(n)
}

/// Set a tag's interest level (interesting / neutral / uninteresting). `star` stays interesting.
#[server]
pub async fn set_tag_interest(name: String, interest: String) -> Result<(), ServerFnError> {
    if !valid_interest(&interest) {
        return Err(ServerFnError::new("invalid interest level"));
    }
    if name == "star" && interest != "interesting" {
        return Err(ServerFnError::new("the ★ tag stays interesting"));
    }
    let conn = open_user()?;
    conn.execute("UPDATE tags SET interest = ?1 WHERE name = ?2", rusqlite::params![interest, name])
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(())
}

/// Convert a tag between a plain boolean and a 1..scale_max scale. Shrinking a
/// scale clamps any stored ratings above the new ceiling (bool clamps them to 1).
/// `star` stays boolean.
#[server]
pub async fn set_tag_scale(name: String, kind: String, scale_max: i64) -> Result<(), ServerFnError> {
    if name == "star" && kind == "scale" {
        return Err(ServerFnError::new("the ★ tag can't be a scale"));
    }
    let (kind, scale_max) = norm_kind(&kind, scale_max);
    let conn = open_user()?;
    // Clamp existing ratings to the new ceiling (never touches 0/considered rows).
    conn.execute(
        "UPDATE word_tags SET value = ?1 WHERE tag = ?2 AND value > ?1",
        rusqlite::params![scale_max, name],
    )
    .map_err(|e| ServerFnError::new(e.to_string()))?;
    conn.execute(
        "UPDATE tags SET kind = ?1, scale_max = ?2 WHERE name = ?3",
        rusqlite::params![kind, scale_max, name],
    )
    .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(())
}

/// How many words a tag is applied to *globally* (book_slug='*') — used to warn
/// before demoting a word-scoped tag back to book scope.
#[server]
pub async fn tag_global_usage(name: String) -> Result<i64, ServerFnError> {
    let conn = open_user()?;
    conn.query_row(
        "SELECT count(*) FROM word_tags WHERE tag = ?1 AND book_slug = '*'",
        [&name],
        |r| r.get(0),
    )
    .map_err(|e| ServerFnError::new(e.to_string()))
}

/// Change a tag's scope and migrate its applications.
///   book→word : collapse the tag's book-scoped rows into the global '*' sentinel.
///   word→book : `convert_slug` empty ⇒ drop the global rows; non-empty ⇒ re-home
///               them to that book (the "just added to global, moved straight back"
///               case the client handles silently). Otherwise the client confirms first.
#[server]
pub async fn set_tag_scope(name: String, scope: String, convert_book: i64) -> Result<(), ServerFnError> {
    use rusqlite::OptionalExtension;
    if !valid_scope(&scope) {
        return Err(ServerFnError::new("invalid scope"));
    }
    // open_conn (dict + ATTACH u) so we can resolve convert_book -> slug from books.
    let mut conn = open_conn()?;
    let convert_slug: Option<String> = if convert_book > 0 {
        conn.query_row("SELECT slug FROM books WHERE id = ?1", [convert_book], |r| r.get(0))
            .optional()
            .map_err(|e| ServerFnError::new(e.to_string()))?
    } else {
        None
    };
    let tx = conn.transaction().map_err(|e| ServerFnError::new(e.to_string()))?;
    if scope == SCOPE_WORD {
        // book→word: collapse this tag's book-scoped rows into the global '*' sentinel.
        // `value` is carried so scale ratings / 0-considered rows survive the move.
        tx.execute(
            // collapse to one '*' row per (word,tag,rater); applied-wins on `value`.
            "INSERT OR IGNORE INTO u.word_tags(book_slug, word, tag, rater, ts, value)
             SELECT '*', word, tag, rater, MAX(ts), MAX(COALESCE(value, 1))
             FROM u.word_tags WHERE tag = ?1 AND book_slug <> '*'
             GROUP BY word, tag, rater",
            [&name],
        )
        .map_err(|e| ServerFnError::new(e.to_string()))?;
        tx.execute("DELETE FROM u.word_tags WHERE tag = ?1 AND book_slug <> '*'", [&name])
            .map_err(|e| ServerFnError::new(e.to_string()))?;
    } else if let Some(slug) = convert_slug {
        // word→book, "just added": re-home the global rows to the current book.
        tx.execute(
            "INSERT OR IGNORE INTO u.word_tags(book_slug, word, tag, rater, ts, value)
             SELECT ?2, word, tag, rater, ts, value FROM u.word_tags WHERE tag = ?1 AND book_slug = '*'",
            rusqlite::params![name, slug],
        )
        .map_err(|e| ServerFnError::new(e.to_string()))?;
        tx.execute("DELETE FROM u.word_tags WHERE tag = ?1 AND book_slug = '*'", [&name])
            .map_err(|e| ServerFnError::new(e.to_string()))?;
    } else {
        // word→book, confirmed drop: discard the global applications.
        tx.execute("DELETE FROM u.word_tags WHERE tag = ?1 AND book_slug = '*'", [&name])
            .map_err(|e| ServerFnError::new(e.to_string()))?;
    }
    tx.execute("UPDATE u.tags SET scope = ?1 WHERE name = ?2", rusqlite::params![scope, name])
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    tx.commit().map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(())
}

/// Apply a scope group's full drag-drop layout: `items` is the scope's tags in the
/// new display order as (name, section). Renumbers `sort = index` and sets each
/// tag's `section`, in one transaction. Expresses reorder AND section reassignment.
/// Only rows actually in `scope` are touched (a stale name is ignored).
#[server]
pub async fn set_scope_layout(scope: String, items: Vec<(String, String)>) -> Result<(), ServerFnError> {
    if !valid_scope(&scope) {
        return Err(ServerFnError::new("invalid scope"));
    }
    let mut conn = open_user()?;
    let tx = conn.transaction().map_err(|e| ServerFnError::new(e.to_string()))?;
    for (i, (name, section)) in items.iter().enumerate() {
        let section = sanitize_section(section);
        tx.execute(
            "UPDATE tags SET sort = ?1, section = ?2 WHERE name = ?3 AND scope = ?4",
            rusqlite::params![i as i64, section, name, scope],
        )
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    }
    tx.commit().map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(())
}

use server_fn::codec::{MultipartData, MultipartFormData};

/// Drag-drop upload: stream the file to a staging path, then inspect it (extract
/// text + metadata, segment kept/stripped regions, dedup check) WITHOUT committing.
#[server(input = MultipartFormData)]
pub async fn upload_book(data: MultipartData) -> Result<Inspection, ServerFnError> {
    let mut data = data.into_inner().ok_or_else(|| ServerFnError::new("no upload body"))?;
    let mut filename = String::new();
    let mut bytes: Vec<u8> = Vec::new();
    while let Some(mut field) =
        data.next_field().await.map_err(|e| ServerFnError::new(e.to_string()))?
    {
        if field.name() == Some("file") {
            filename = field.file_name().unwrap_or("book").to_string();
            while let Some(chunk) =
                field.chunk().await.map_err(|e| ServerFnError::new(e.to_string()))?
            {
                bytes.extend_from_slice(&chunk);
            }
        }
    }
    if bytes.is_empty() {
        return Err(ServerFnError::new("no file received"));
    }
    let ext = std::path::Path::new(&filename)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .filter(|e| !e.is_empty() && e.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or_else(|| "txt".to_string());
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    let token = format!("up-{:016x}.{ext}", h.finish());
    let staged = staging_dir().join(&token);
    std::fs::write(&staged, &bytes).map_err(|e| ServerFnError::new(format!("staging write: {e}")))?;

    let v = run_importer(&["--inspect", &staged.to_string_lossy()])?;
    let mut insp: Inspection =
        serde_json::from_value(v).map_err(|e| ServerFnError::new(format!("parse inspection: {e}")))?;
    insp.token = token;
    if insp.orig_filename.is_empty() {
        insp.orig_filename = filename;
    }
    Ok(insp)
}

/// Commit a previously-uploaded file: copy it into the books dir, ingest the word
/// histogram, then run the per-book analysis pipeline (score, cluster, trajectory).
#[server]
pub async fn confirm_import(
    token: String,
    slug: String,
    title: String,
    author: String,
    year: String,
    orig_filename: String,
) -> Result<ImportResult, ServerFnError> {
    if token.is_empty() || token.contains('/') || token.contains('\\') || token.contains("..") {
        return Err(ServerFnError::new("invalid upload token"));
    }
    let slug = sanitize_slug(&slug);
    if slug.is_empty() {
        return Err(ServerFnError::new("please enter a slug (letters, numbers, dashes)"));
    }
    let staged = staging_dir().join(&token);
    if !staged.exists() {
        return Err(ServerFnError::new("upload expired — please drop the file again"));
    }
    // Import is always fast (embedded text for PDFs); OCR is done later on /books.
    let mut args: Vec<String> = vec![
        "--commit".into(),
        staged.to_string_lossy().to_string(),
        "--slug".into(),
        slug,
        "--title".into(),
        title,
        "--author".into(),
        author,
        "--orig-filename".into(),
        orig_filename,
    ];
    let y = year.trim();
    if !y.is_empty() {
        args.push("--year".into());
        args.push(y.to_string());
    }
    let argref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let v = run_importer(&argref)?;
    let res: ImportResult =
        serde_json::from_value(v).map_err(|e| ServerFnError::new(format!("parse result: {e}")))?;
    let _ = std::fs::remove_file(&staged);
    Ok(res)
}

// ---- book management (/books): admin + OCR/source jobs ---- #

/// (slug, extension) for a book id, e.g. ("gutenberg-2701", "txt").
#[cfg(feature = "ssr")]
fn book_slug_ext(conn: &rusqlite::Connection, book_id: i64) -> Result<(String, String), ServerFnError> {
    use rusqlite::OptionalExtension;
    let row: Option<(String, Option<String>)> = conn
        .query_row("SELECT slug, format FROM books WHERE id = ?1", [book_id], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let (slug, fmt) = row.ok_or_else(|| ServerFnError::new("book not found"))?;
    let ext = match fmt.as_deref() {
        Some("epub") => "epub",
        Some("pdf") => "pdf",
        _ => "txt",
    };
    Ok((slug, ext.to_string()))
}

/// All books with the fields the management page edits/shows.
#[server]
pub async fn list_books_admin() -> Result<Vec<BookAdmin>, ServerFnError> {
    let conn = open_conn()?;
    let mut stmt = conn
        .prepare(
            "SELECT b.id, b.slug, COALESCE(b.title,''), COALESCE(b.author,''), b.year,
                    COALESCE(b.format,''), COALESCE(b.source,''), COALESCE(b.text_source,''),
                    COALESCE(b.n_tokens,0), COALESCE(b.n_types,0), COALESCE(b.ingested_at,''),
                    (SELECT count(DISTINCT t.word) FROM u.word_tags t
                       JOIN u.tags g ON (g.name=t.tag OR t.tag LIKE g.name || '.%')
                     LEFT JOIN words w ON w.word=t.word
                     LEFT JOIN book_occurrences bo ON bo.word_id=w.id AND bo.book_id=b.id
                     WHERE g.interest='interesting' AND COALESCE(t.value,1) >= 1
                       AND (t.book_slug=b.slug OR (t.book_slug='*' AND bo.word_id IS NOT NULL)))
             FROM books b ORDER BY b.id",
        )
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(BookAdmin {
                id: r.get(0)?,
                slug: r.get(1)?,
                title: r.get(2)?,
                author: r.get(3)?,
                year: r.get(4)?,
                format: r.get(5)?,
                source: r.get(6)?,
                text_source: r.get(7)?,
                n_tokens: r.get(8)?,
                n_types: r.get(9)?,
                ingested_at: r.get(10)?,
                n_selected: r.get(11)?,
            })
        })
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| ServerFnError::new(e.to_string()))?);
    }
    Ok(out)
}

/// Edit a book's display details (not the slug — it keys tags + the stored file).
#[server]
pub async fn update_book(book_id: i64, title: String, author: String, year: String) -> Result<(), ServerFnError> {
    let yr: Option<i64> = year.trim().parse().ok().filter(|y| (1000..=2200).contains(y));
    let conn = rusqlite::Connection::open(db_path()).map_err(|e| ServerFnError::new(e.to_string()))?;
    conn.execute(
        "UPDATE books SET title=?1, author=?2, year=?3 WHERE id=?4",
        rusqlite::params![title.trim(), author.trim(), yr, book_id],
    )
    .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(())
}

/// Delete a book: its rows + stored file + OCR sidecars. Tags (text-keyed in
/// user.db) are left dormant and resurrect if the book is re-imported.
#[server]
pub async fn delete_book(book_id: i64) -> Result<(), ServerFnError> {
    let conn = rusqlite::Connection::open(db_path()).map_err(|e| ServerFnError::new(e.to_string()))?;
    let (slug, ext) = book_slug_ext(&conn, book_id)?;
    for t in ["book_occurrences", "candidates", "ratings"] {
        let _ = conn.execute(&format!("DELETE FROM {t} WHERE book_id=?1"), [book_id]);
    }
    conn.execute("DELETE FROM books WHERE id=?1", [book_id])
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    // remove the committed file + any <slug>.<ext>.ocr.*.json sidecars
    let dir = books_dir();
    let _ = std::fs::remove_file(dir.join(format!("{slug}.{ext}")));
    if let Ok(rd) = std::fs::read_dir(&dir) {
        let prefix = format!("{slug}.{ext}.ocr.");
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().starts_with(&prefix) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    Ok(())
}

/// PDF OCR / text-source state for a book (engine cache sizes, page counts).
#[server]
pub async fn book_ocr_status(book_id: i64) -> Result<OcrStatus, ServerFnError> {
    let conn = rusqlite::Connection::open(db_path()).map_err(|e| ServerFnError::new(e.to_string()))?;
    let (slug, _) = book_slug_ext(&conn, book_id)?;
    let v = run_importer(&["--ocr-status", &slug])?;
    serde_json::from_value(v).map_err(|e| ServerFnError::new(format!("parse status: {e}")))
}

/// Compare a committed PDF's embedded text vs re-OCR on sampled pages (book-keyed cache).
#[server]
pub async fn ocr_compare_book(book_id: i64, engine: String) -> Result<OcrCompareResult, ServerFnError> {
    let conn = rusqlite::Connection::open(db_path()).map_err(|e| ServerFnError::new(e.to_string()))?;
    let (slug, ext) = book_slug_ext(&conn, book_id)?;
    let path = books_dir().join(format!("{slug}.{ext}"));
    let path = path.to_string_lossy().to_string();
    let mut args: Vec<String> = vec!["--ocr-compare".into(), path];
    if !engine.is_empty() {
        args.push("--engine".into());
        args.push(engine);
    }
    let argref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let v = run_importer(&argref)?;
    serde_json::from_value(v).map_err(|e| ServerFnError::new(format!("parse compare: {e}")))
}

/// Delete one engine's OCR cache for a book.
#[server]
pub async fn delete_ocr(book_id: i64, engine: String) -> Result<(), ServerFnError> {
    if !engine.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(ServerFnError::new("bad engine"));
    }
    let conn = rusqlite::Connection::open(db_path()).map_err(|e| ServerFnError::new(e.to_string()))?;
    let (slug, ext) = book_slug_ext(&conn, book_id)?;
    let p = books_dir().join(format!("{slug}.{ext}.ocr.{engine}.json"));
    let _ = std::fs::remove_file(p);
    Ok(())
}

/// Start a background OCR job for a book+engine; returns the (deduped) job id.
#[server]
pub async fn start_ocr(book_id: i64, engine: String) -> Result<String, ServerFnError> {
    let conn = rusqlite::Connection::open(db_path()).map_err(|e| ServerFnError::new(e.to_string()))?;
    let (slug, _) = book_slug_ext(&conn, book_id)?;
    let eng = if engine.is_empty() { "auto".to_string() } else { engine };
    let mut args = vec!["--ocr-book".to_string(), slug];
    if eng != "auto" {
        args.push("--engine".into());
        args.push(eng.clone());
    }
    Ok(crate::jobs::start("ocr", book_id, &eng, &format!("OCR ({eng})…"), args))
}

/// Start a background re-ingest from a chosen text source (embedded | ocr:<engine>).
#[server]
pub async fn start_reingest(book_id: i64, source: String) -> Result<String, ServerFnError> {
    let conn = rusqlite::Connection::open(db_path()).map_err(|e| ServerFnError::new(e.to_string()))?;
    let (slug, _) = book_slug_ext(&conn, book_id)?;
    let args = vec!["--reingest".to_string(), slug, "--text-source".into(), source.clone()];
    Ok(crate::jobs::start("reingest", book_id, &source, &format!("switching to {source}…"), args))
}

/// Start a background global usage-chart (trajectory) refresh.
#[server]
pub async fn refresh_trajectory() -> Result<String, ServerFnError> {
    Ok(crate::jobs::start("trajectory", 0, "", "refreshing usage charts…",
        vec!["--refresh-trajectory".to_string()]))
}

#[server]
pub async fn job_status(id: String) -> Result<Option<JobProgress>, ServerFnError> {
    Ok(crate::jobs::status(&id))
}

#[server]
pub async fn cancel_job(id: String) -> Result<(), ServerFnError> {
    crate::jobs::cancel(&id);
    Ok(())
}

/// Re-segment an already-imported book's stored file, for the "view stripping" page.
#[server]
pub async fn view_source(book_id: i64) -> Result<Inspection, ServerFnError> {
    use rusqlite::OptionalExtension;
    let conn = rusqlite::Connection::open(db_path()).map_err(|e| ServerFnError::new(e.to_string()))?;
    let row: Option<(String, Option<String>)> = conn
        .query_row("SELECT slug, format FROM books WHERE id = ?1", [book_id], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let Some((slug, fmt)) = row else {
        return Err(ServerFnError::new("book not found"));
    };
    let ext = match fmt.as_deref() {
        Some("epub") => "epub",
        Some("pdf") => "pdf",
        _ => "txt",
    };
    // UI-imported books live in BOOKS_DIR as <slug>.<ext>; older CLI-imported ones
    // may still be in the repo's data/books as <slug>.txt/.epub — try both.
    let root = repo_root();
    let dbooks = std::path::Path::new(&root).join("data").join("books");
    let candidates = [
        books_dir().join(format!("{slug}.{ext}")),
        dbooks.join(format!("{slug}.txt")),
        dbooks.join(format!("{slug}.epub")),
    ];
    let Some(path) = candidates.into_iter().find(|p| p.exists()) else {
        return Err(ServerFnError::new(format!(
            "no stored source file for '{slug}' — re-import it via the web UI to view its stripping"
        )));
    };
    // run_importer runs Python with cwd=repo_root, so a path relative to the
    // server's own cwd would resolve wrong — make it absolute first.
    let path = std::fs::canonicalize(&path).unwrap_or(path);
    let v = run_importer(&["--inspect", &path.to_string_lossy()])?;
    serde_json::from_value(v).map_err(|e| ServerFnError::new(format!("parse: {e}")))
}

// ---- client-side tag helpers (operate on the shared Tagger context) ----
/// The stored tri-state value for a tag on a word: None = never considered,
/// Some(0) = considered & deliberately declined, Some(>=1) = applied.
fn tag_value(t: Tagger, key: (i64, i64), tag: &str) -> Option<i32> {
    t.store.with(|m| m.get(&key).and_then(|s| s.get(tag).copied()))
}
fn has_tag(t: Tagger, key: (i64, i64), tag: &str) -> bool {
    tag_value(t, key, tag).is_some_and(|v| v >= 1)
}
/// A tag counts as "on" for a word if directly applied OR any applied descendant
/// implies it (child-implies-parent). Drives the parent chip's lit/implied state.
fn implied_on(t: Tagger, key: (i64, i64), tag: &str) -> bool {
    t.store.with(|m| {
        m.get(&key).is_some_and(|s| {
            s.iter().any(|(name, &v)| v >= 1 && (name == tag || is_ancestor(tag, name)))
        })
    })
}
fn has_other_tags(t: Tagger, key: (i64, i64)) -> bool {
    t.store.with(|m| m.get(&key).is_some_and(|s| s.iter().any(|(x, &v)| v >= 1 && x != "star")))
}
/// Any in-book member of the group carries a non-star tag (drives the per-row
/// "tags" button highlight regardless of which level introduced the tag).
fn group_has_other(t: Tagger, book_id: i64, members: &[i64]) -> bool {
    members.iter().any(|&w| has_other_tags(t, (book_id, w)))
}
fn toggle_tag(t: Tagger, book_id: i64, word_id: i64, tag: &str) {
    let key = (book_id, word_id);
    let next = !has_tag(t, key, tag);
    t.store.update(|m| {
        let set = m.entry(key).or_default();
        if next { set.insert(tag.to_string(), 1); } else { set.remove(tag); }
    });
    t.action.dispatch(SetTag { book_id, word_id, tag: tag.to_string(), on: next });
}
/// Set a tag's tri-state value on a word (scale ratings + the 0/considered state).
/// `value` None removes the row (untagged); Some(0) records considered-declined;
/// Some(n) applies at level n. Optimistic store update + server dispatch.
fn set_tag_val(t: Tagger, book_id: i64, word_id: i64, tag: &str, value: Option<i32>) {
    let key = (book_id, word_id);
    t.store.update(|m| {
        let set = m.entry(key).or_default();
        match value {
            Some(v) => { set.insert(tag.to_string(), v); }
            None => { set.remove(tag); }
        }
    });
    t.set_val.dispatch(SetTagValue { book_id, word_id, tag: tag.to_string(), value });
}

/// The book the user last looked at, shared across pages so the nav "words" link
/// (and a bare `/`) returns you where you were rather than to the first book.
/// Backed by localStorage so it survives reloads; updated by the home page.
#[derive(Clone, Copy)]
struct CurrentBook(RwSignal<Option<i64>>);

/// Recall / remember the last-viewed book id via localStorage. Client-only: the
/// `book` Memo reads `stored_book()` during SSR too, and js-sys panics ("cannot
/// access imported statics on non-wasm targets") if `web_sys` is touched on the
/// native server — so these are no-ops off the hydrate (wasm) target.
#[cfg(feature = "hydrate")]
fn stored_book() -> Option<i64> {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item("coolwords.book").ok().flatten())
        .and_then(|v| v.parse::<i64>().ok())
}
#[cfg(not(feature = "hydrate"))]
fn stored_book() -> Option<i64> {
    None
}
#[cfg(feature = "hydrate")]
fn remember_book(id: i64) {
    if let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = s.set_item("coolwords.book", &id.to_string());
    }
}
#[cfg(not(feature = "hydrate"))]
fn remember_book(_id: i64) {}

/// Persistent top menu shown on every page: brand + the three sections + a single
/// consistent "import" action. The active page is highlighted, and "words" carries
/// the current book so you land back where you left off.
#[component]
fn NavBar() -> impl IntoView {
    let current = expect_context::<CurrentBook>();
    let path = use_location().pathname;
    let ibase = base_path();
    let words_href = {
        let b = ibase.clone();
        move || match current.0.get() {
            Some(id) => format!("{b}/?book={id}"),
            None => format!("{b}/"),
        }
    };
    // `path` is the raw browser pathname (includes the ingress prefix), so strip the
    // base before comparing against the app-relative route to set the active class.
    let cls = move |p: &'static str, klass: &'static str| {
        let b = ibase.clone();
        move || {
            let full = path.get();
            let rel = full.strip_prefix(b.as_str()).unwrap_or(&full);
            let rel = if rel.is_empty() { "/" } else { rel };
            if rel == p { format!("{klass} active") } else { klass.to_string() }
        }
    };
    let base = base_path();
    view! {
        <nav class="navbar">
            <span class="brand">"coolwords"</span>
            <A href=words_href attr:class=cls("/", "navlink")>"words"</A>
            <A href=format!("{base}/collection") attr:class=cls("/collection", "navlink")>"collection"</A>
            <A href=format!("{base}/books") attr:class=cls("/books", "navlink")>"books"</A>
            <A href=format!("{base}/tags") attr:class=cls("/tags", "navlink")>"tags"</A>
            <A href=format!("{base}/import") attr:class=cls("/import", "navimport")>"+ import book"</A>
        </nav>
    }
}

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();
    // Same value on the server (from the header) and the client (from <html data-base>),
    // so hydration matches. Shared with the rest of the tree via context.
    let base = ingress_base();
    provide_context(BasePath(base.clone()));
    let current = CurrentBook(RwSignal::new(None));
    provide_context(current);
    // Transient toast (tag comment on touch), shared by the picker + tags page.
    provide_context(Toast::new());
    // Seed the shared "current book" from localStorage on first load (client-only).
    Effect::new(move |_| {
        if let Some(id) = stored_book() {
            current.0.set(Some(id));
        }
    });
    view! {
        <Stylesheet id="leptos" href=format!("{base}/pkg/coolwords_ui.css")/>
        <Title text="coolwords — interesting words"/>
        <Router base=base>
            <main>
                <NavBar/>
                <Routes fallback=|| "Page not found.".into_view()>
                    <Route path=StaticSegment("") view=HomePage/>
                    <Route path=StaticSegment("collection") view=CollectionPage/>
                    <Route path=StaticSegment("tags") view=TagsPage/>
                    <Route path=StaticSegment("books") view=BooksAdminPage/>
                    <Route path=StaticSegment("import") view=ImportPage/>
                    <Route path=StaticSegment("source") view=BookSourcePage/>
                </Routes>
                <ToastView/>
            </main>
        </Router>
    }
}

fn short(s: &Option<String>, n: usize) -> String {
    match s {
        None => String::new(),
        Some(v) => {
            let t: String = v.chars().take(n).collect();
            if v.chars().count() > n { format!("{t}…") } else { t }
        }
    }
}

fn highlight(text: &str, word: &str) -> Vec<(String, bool)> {
    let w = word.to_lowercase();
    let hay = text.to_lowercase();
    let hb = hay.as_bytes();
    let mut segs = Vec::new();
    if w.is_empty() {
        segs.push((text.to_string(), false));
        return segs;
    }
    let (mut last, mut from) = (0usize, 0usize);
    while let Some(rel) = hay[from..].find(&w) {
        let start = from + rel;
        let end = start + w.len();
        let before_ok = start == 0 || !hb[start - 1].is_ascii_alphanumeric();
        let after_ok = end == hb.len() || !hb[end].is_ascii_alphanumeric();
        if before_ok && after_ok {
            if start > last {
                segs.push((text[last..start].to_string(), false));
            }
            segs.push((text[start..end].to_string(), true));
            last = end;
        }
        from = end;
    }
    if last < text.len() {
        segs.push((text[last..].to_string(), false));
    }
    if segs.is_empty() {
        segs.push((text.to_string(), false));
    }
    segs
}

/// A quick ★ favourite toggle for a word.
#[component]
fn Star(book_id: i64, word_id: i64) -> impl IntoView {
    let t = expect_context::<Tagger>();
    let key = (book_id, word_id);
    view! {
        <button type="button" class="star" class:on=move || has_tag(t, key, "star")
            title="favourite" on:click=move |_| toggle_tag(t, book_id, word_id, "star")>"★"</button>
    }
}

/// CSS class fragment for an interest level (matches the `.int-*` colour rules).
fn interest_class(i: Interest) -> &'static str {
    match i {
        Interest::Interesting => "interesting",
        Interest::Neutral => "neutral",
        Interest::Uninteresting => "uninteresting",
    }
}

/// The applied-tags strip shown on a word card: the family's own tags as small
/// interest-coloured labels, collapsing to per-kind count pills when there are too
/// many to list (green = interesting, grey = neutral, red = uninteresting).
#[component]
fn TagStrip(book_id: i64, members: Vec<i64>) -> impl IntoView {
    let t = expect_context::<Tagger>();
    // How many labels before we collapse to counts.
    const MAX_CHIPS: usize = 3;
    move || {
        let tags = group_tags(t, book_id, &members);
        if tags.is_empty() {
            return ().into_any();
        }
        if tags.len() <= MAX_CHIPS {
            return view! {
                <span class="tagstrip">
                    {tags.into_iter().map(|(n, i)| {
                        let cls = format!("taglabel int-{}", interest_class(i));
                        view! { <span class=cls>{n}</span> }
                    }).collect_view()}
                </span>
            }.into_any();
        }
        let (g, ne, r) = group_tag_counts(t, book_id, &members);
        view! {
            <span class="tagsummary" title="tags on this word (by kind)">
                {(g > 0).then(|| view! { <span class="tagcount int-interesting">{g}</span> })}
                {(ne > 0).then(|| view! { <span class="tagcount int-neutral">{ne}</span> })}
                {(r > 0).then(|| view! { <span class="tagcount int-uninteresting">{r}</span> })}
            </span>
        }.into_any()
    }
}

/// Where a dragged chip/row was dropped within its scope group.
#[derive(Clone)]
enum DropAt {
    /// Immediately before this tag (adopting that tag's section).
    Before(String),
    /// Into this named section (appended to the section's run).
    Section(String),
    /// The end of the scope group (adopting the last tag's section).
    End,
}

/// A scope's tags (incl. ★) as (name, section) in current display order.
fn scope_layout(tags: &[TagDef], is_word: bool) -> Vec<(String, String)> {
    tags.iter()
        .filter(|d| (d.scope == SCOPE_WORD) == is_word)
        .map(|d| (d.name.clone(), d.section.clone()))
        .collect()
}

/// Fold a scope's ordered tags into contiguous (section, tags) runs. A run's
/// heading is its section label ('' = ungrouped, rendered without a heading).
fn section_runs(tags: Vec<TagDef>) -> Vec<(String, Vec<TagDef>)> {
    let mut out: Vec<(String, Vec<TagDef>)> = Vec::new();
    for d in tags {
        match out.last_mut() {
            Some((s, items)) if *s == d.section => items.push(d),
            _ => out.push((d.section.clone(), vec![d])),
        }
    }
    out
}

/// New (name, section) order for a scope after dropping `dragged` at `drop`. The
/// dragged tag adopts the section of wherever it lands, keeping section runs contiguous.
fn compute_reorder(mut list: Vec<(String, String)>, dragged: &str, drop: DropAt) -> Vec<(String, String)> {
    let Some(pos) = list.iter().position(|(n, _)| n == dragged) else { return list };
    let (dname, _) = list.remove(pos);
    let (idx, sect) = match drop {
        DropAt::Before(t) => {
            let p = list.iter().position(|(n, _)| n == &t).unwrap_or(list.len());
            let s = list.get(p).map(|(_, s)| s.clone()).unwrap_or_default();
            (p, s)
        }
        DropAt::Section(s) => match list.iter().rposition(|(_, sc)| sc == &s) {
            Some(p) => (p + 1, s),
            None => (list.len(), s),
        },
        DropAt::End => (list.len(), list.last().map(|(_, s)| s.clone()).unwrap_or_default()),
    };
    list.insert(idx, (dname, sect));
    list
}

/// Apply a drag-drop within a scope: optimistically reorder `tags` + set the
/// dragged tag's section, then persist via `set_scope_layout`. Cross-scope drops
/// are ignored (scope changes only happen on the manage page). Shared by the
/// picker and the manage page (each passes its own tags signal + layout action).
fn do_drop(
    tags: RwSignal<Vec<TagDef>>,
    layout: ServerAction<SetScopeLayout>,
    is_word: bool,
    dragged: String,
    drop: DropAt,
) {
    let same = tags.with(|v| {
        v.iter().find(|d| d.name == dragged).map(|d| (d.scope == SCOPE_WORD) == is_word).unwrap_or(false)
    });
    if !same {
        return;
    }
    let list = tags.with(|v| scope_layout(v, is_word));
    let new_list = compute_reorder(list, &dragged, drop);
    apply_layout(tags, layout, is_word, new_list);
}

/// Optimistically apply a scope's new `(name, section)` order to `tags` (reordering
/// that scope's slots + updating each tag's section), then persist via `layout`.
/// Shared by drag-drop, section reassignment, and the sort controls.
fn apply_layout(
    tags: RwSignal<Vec<TagDef>>,
    layout: ServerAction<SetScopeLayout>,
    is_word: bool,
    new_list: Vec<(String, String)>,
) {
    tags.update(|v| {
        let sect: HashMap<String, String> = new_list.iter().cloned().collect();
        for d in v.iter_mut() {
            if let Some(s) = sect.get(&d.name) {
                d.section = s.clone();
            }
        }
        let slots: Vec<usize> = v.iter().enumerate()
            .filter(|(_, d)| (d.scope == SCOPE_WORD) == is_word).map(|(i, _)| i).collect();
        let byname: HashMap<String, TagDef> =
            slots.iter().map(|&i| (v[i].name.clone(), v[i].clone())).collect();
        for (&slot, (name, _)) in slots.iter().zip(new_list.iter()) {
            if let Some(td) = byname.get(name) {
                v[slot] = td.clone();
            }
        }
    });
    let scope = if is_word { SCOPE_WORD } else { SCOPE_BOOK };
    layout.dispatch(SetScopeLayout { scope: scope.into(), items: new_list });
}

/// Interest ordering for the "by interest" sort: interesting < neutral < uninteresting.
fn interest_rank(i: &str) -> u8 {
    match i {
        "interesting" => 0,
        "neutral" => 1,
        _ => 2,
    }
}

/// Sort each of a scope's section runs in place (keeping the runs — and their
/// order — intact), alphabetically or by interest-then-alphabetically, then persist.
/// `star` is pinned to the front of its run so the quick ★ stays reachable.
fn sort_scope(
    tags: RwSignal<Vec<TagDef>>,
    layout: ServerAction<SetScopeLayout>,
    is_word: bool,
    by_interest: bool,
) {
    let scoped: Vec<TagDef> = tags.with(|v| {
        v.iter().filter(|d| (d.scope == SCOPE_WORD) == is_word).cloned().collect()
    });
    let new_list: Vec<(String, String)> = section_runs(scoped)
        .into_iter()
        .flat_map(|(section, mut items)| {
            items.sort_by(|a, b| {
                let star = (b.name == "star").cmp(&(a.name == "star"));
                let key = if by_interest {
                    interest_rank(&a.interest).cmp(&interest_rank(&b.interest))
                } else {
                    std::cmp::Ordering::Equal
                };
                star.then(key).then_with(|| a.name.cmp(&b.name))
            });
            let s = section.clone();
            items.into_iter().map(move |d| (d.name, s.clone()))
        })
        .collect();
    apply_layout(tags, layout, is_word, new_list);
}

/// Reassign a tag's section without changing order (the per-row section input on
/// the manage page): patch the tag in `tags` optimistically, then persist the
/// scope's whole layout so the new section sticks.
fn set_section(
    tags: RwSignal<Vec<TagDef>>,
    layout: ServerAction<SetScopeLayout>,
    name: String,
    is_word: bool,
    section: String,
) {
    let section = sanitize_section(&section);
    let list = tags.with(|v| scope_layout(v, is_word));
    // Filing a tag under an existing section name should MERGE it into that
    // section's run — otherwise `section_runs` (which folds by adjacency) renders a
    // second heading with the same name. `compute_reorder(Section)` moves the tag to
    // the end of the matching run (or appends a new run if none exists yet).
    // The ungrouped ('') case just relabels in place — don't yank it to the end.
    let new_list = if section.is_empty() {
        list.into_iter()
            .map(|(n, s)| if n == name { (n, String::new()) } else { (n, s) })
            .collect()
    } else {
        compute_reorder(list, &name, DropAt::Section(section))
    };
    apply_layout(tags, layout, is_word, new_list);
}

// ---- touch-capable drag (Pointer Events) ----

/// How far (px) the pointer must travel before a press becomes a drag rather than a
/// tap. Below this, pointerup is a plain tap (toggle / select).
const DRAG_SLOP: f64 = 8.0;

/// Per-surface pointer-drag state. HTML5 drag-and-drop never fires on touch, so
/// reordering rides on Pointer Events: a press records `pending`; the first move
/// past DRAG_SLOP promotes it to an `active` drag; `over` tracks the tag or section
/// under the pointer for the eventual drop.
#[derive(Clone, Copy)]
struct DragState {
    /// Press origin (x, y) + the pressed tag name, before the slop threshold.
    pending: RwSignal<Option<(f64, f64, String)>>,
    /// The tag being dragged once the slop threshold is crossed.
    active: RwSignal<Option<String>>,
    /// (target, is_section) currently under the pointer — the pending drop.
    over: RwSignal<Option<(String, bool)>>,
}

impl DragState {
    fn new() -> Self {
        Self {
            pending: RwSignal::new(None),
            active: RwSignal::new(None),
            over: RwSignal::new(None),
        }
    }
    fn reset(&self) {
        self.pending.set(None);
        self.active.set(None);
        self.over.set(None);
    }
}

/// The drag target under a screen point: the nearest ancestor carrying either a
/// `data-section` (a section heading) or `data-tagname` (a chip/row). Returns
/// (name, is_section).
fn drag_target_at(x: f64, y: f64) -> Option<(String, bool)> {
    let doc = web_sys::window()?.document()?;
    let el = doc.element_from_point(x as f32, y as f32)?;
    let hit = el.closest("[data-section],[data-tagname]").ok().flatten()?;
    if let Some(sec) = hit.get_attribute("data-section") {
        Some((sec, true))
    } else {
        hit.get_attribute("data-tagname").map(|n| (n, false))
    }
}

/// Nudge the window when the pointer nears the top/bottom edge, so a drag can cross
/// a list taller than the viewport (there's no native autoscroll in a manual drag).
fn edge_autoscroll(y: f64) {
    if let Some(w) = web_sys::window() {
        let h = w.inner_height().ok().and_then(|v| v.as_f64()).unwrap_or(0.0);
        if y < 64.0 {
            let _ = w.scroll_by_with_x_and_y(0.0, -16.0);
        } else if h > 0.0 && y > h - 64.0 {
            let _ = w.scroll_by_with_x_and_y(0.0, 16.0);
        }
    }
}

/// Capture the pointer on the element the handler is bound to, so move/up keep
/// firing after the finger slides off it (essential for cross-element drags).
fn capture_pointer(ev: &web_sys::PointerEvent) {
    use wasm_bindgen::JsCast;
    if let Some(el) = ev
        .current_target()
        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
    {
        let _ = el.set_pointer_capture(ev.pointer_id());
    }
}

/// Resolve a finished drag into a DropAt for the dragged tag: onto a section
/// heading → into that section; onto another tag → before it; onto itself → no
/// move; onto nothing → the end of the scope.
fn resolve_drop(dragged: &str, over: Option<(String, bool)>) -> Option<DropAt> {
    match over {
        Some((sec, true)) => Some(DropAt::Section(sec)),
        Some((name, false)) if name != dragged => Some(DropAt::Before(name)),
        Some(_) => None,
        None => Some(DropAt::End),
    }
}

/// Build (pointerdown, pointermove, pointerup) handlers that drag tag `name` within
/// scope `is_word`, persisting via `do_drop`. Dragging only engages while `enabled`
/// is true (edit mode). `on_tap` fires when the press ends below the drag threshold
/// (a tap / select). Pair with `on:pointercancel` → reset.
#[allow(clippy::type_complexity)]
fn drag_handlers(
    ds: DragState,
    tags: RwSignal<Vec<TagDef>>,
    layout: ServerAction<SetScopeLayout>,
    enabled: RwSignal<bool>,
    is_word: bool,
    name: String,
    on_tap: impl Fn() + Clone + 'static,
) -> (
    impl Fn(web_sys::PointerEvent) + Clone + 'static,
    impl Fn(web_sys::PointerEvent) + Clone + 'static,
    impl Fn(web_sys::PointerEvent) + Clone + 'static,
) {
    let down = {
        let name = name.clone();
        move |ev: web_sys::PointerEvent| {
            if !enabled.get_untracked() {
                return;
            }
            capture_pointer(&ev);
            ds.pending.set(Some((ev.client_x() as f64, ev.client_y() as f64, name.clone())));
            ds.active.set(None);
            ds.over.set(None);
        }
    };
    let mv = move |ev: web_sys::PointerEvent| {
        let (x, y) = (ev.client_x() as f64, ev.client_y() as f64);
        if ds.active.get_untracked().is_none() {
            if let Some((sx, sy, nm)) = ds.pending.get_untracked() {
                if ((x - sx).powi(2) + (y - sy).powi(2)).sqrt() > DRAG_SLOP {
                    ds.active.set(Some(nm));
                    ds.pending.set(None);
                }
            }
        }
        if ds.active.get_untracked().is_some() {
            ds.over.set(drag_target_at(x, y));
            edge_autoscroll(y);
        }
    };
    let up = move |_ev: web_sys::PointerEvent| {
        if let Some(dragged) = ds.active.get_untracked() {
            if let Some(drop) = resolve_drop(&dragged, ds.over.get_untracked()) {
                do_drop(tags, layout, is_word, dragged, drop);
            }
            ds.reset();
            // flash the moved tag's description too (a drag counts as a "touch")
            on_tap();
        } else {
            let tapped = ds.pending.get_untracked().is_some();
            ds.reset();
            if tapped {
                on_tap();
            }
        }
    };
    (down, mv, up)
}

// ---- transient toast (read a tag's comment on touch, where tooltips are dead) ----

/// A brief bottom-of-screen message, provided at the app root. Touching a tag
/// (tap / toggle / a cancelled drag) flashes its name + comment here — the only way
/// to read a tag's description on touch, where the title tooltip never appears.
#[derive(Clone, Copy)]
pub struct Toast {
    msg: RwSignal<Option<String>>,
    /// Bumped on every show; the auto-dismiss timer only clears if it still matches,
    /// so a later toast isn't wiped by an earlier one's timer.
    gen: RwSignal<u32>,
}

impl Toast {
    fn new() -> Self {
        Self { msg: RwSignal::new(None), gen: RwSignal::new(0) }
    }
    /// Flash `text` for ~2.6s. No-op for empty text.
    fn show(&self, text: String) {
        if text.trim().is_empty() {
            return;
        }
        let g = self.gen.get_untracked().wrapping_add(1);
        self.gen.set(g);
        self.msg.set(Some(text));
        let gen = self.gen;
        let msg = self.msg;
        leptos::prelude::set_timeout(
            move || {
                if gen.get_untracked() == g {
                    msg.set(None);
                }
            },
            std::time::Duration::from_millis(2600),
        );
    }
}

/// Flash a tag's name + comment (from the collection) in the toast.
fn toast_tag(toast: Toast, tags: RwSignal<Vec<TagDef>>, name: &str) {
    let comment = tags.with(|v| {
        v.iter().find(|d| d.name == name).and_then(|d| d.comment.clone())
    });
    let text = match comment {
        Some(c) if !c.trim().is_empty() => format!("{name} — {c}"),
        _ => name.to_string(),
    };
    toast.show(text);
}

/// The toast surface — a fixed pill near the bottom, mounted once at the app root.
#[component]
fn ToastView() -> impl IntoView {
    let toast = expect_context::<Toast>();
    view! {
        {move || toast.msg.get().map(|m| view! { <div class="toast" role="status">{m}</div> })}
    }
}

/// The tag picker: the user's collection split into "this book" (book-scoped) and
/// "all books" (word-scoped) groups, each divided into custom section subheadings.
/// Tap a chip to apply/remove; touching one flashes its description as a toast.
/// Turn on "edit" to drag a chip (touch-friendly) — reorder it or drop it on a
/// subheading to refile it. The adder searches existing tags first, and only
/// reveals scope/interest when you choose to create a brand-new tag. Scope is
/// otherwise changed on the manage page. Plus per-word "good for: <bucket>" picks.
#[component]
fn TagPicker(book_id: i64, word_id: i64, buckets: Vec<String>) -> impl IntoView {
    let t = expect_context::<Tagger>();
    let toast = expect_context::<Toast>();
    let key = (book_id, word_id);
    let has_buckets = !buckets.is_empty();
    let new_name = RwSignal::new(String::new());
    let new_comment = RwSignal::new(String::new());
    let new_word = RwSignal::new(false); // scope: false = this book, true = all books
    let new_interest = RwSignal::new("interesting".to_string());
    let new_kind = RwSignal::new("bool".to_string()); // "bool" | "scale"
    let new_max = RwSignal::new(5i64);                 // scale ceiling when kind == "scale"
    // "descriptions" mode: expand every chip to show + edit its comment inline.
    let describe = RwSignal::new(false);
    // "edit" mode: gates drag-to-reorder so a stray swipe can't reshuffle tags.
    let editing = RwSignal::new(false);
    // "rate" mode: expand every tag to a tri-state / scale strip (⌫ untag, ✗ = 0
    // considered, 1..max = level) so the 0/considered state + scale levels are reachable.
    let rating = RwSignal::new(false);
    // Progressive disclosure: the scope/interest/comment form for creating a NEW tag
    // stays hidden until the user opts in ("create …"), so the common case (search +
    // apply, or a quick default-scoped add) isn't cluttered by two selects.
    let expand = RwSignal::new(false);
    let ds = DragState::new();

    let add_new = move || {
        let raw = new_name.get();
        let Some(clean) = sanitize_tag(&raw) else { return };
        let exists = t.tags.with(|v| v.iter().any(|d| d.name == clean));
        if exists {
            // Re-adding an existing tag only APPLIES it — never re-create it (that
            // would risk clobbering its scope/interest). Just toggle it on.
            if !has_tag(t, key, &clean) {
                toggle_tag(t, book_id, word_id, &clean);
            }
        } else {
            let scope = if new_word.get() { SCOPE_WORD } else { SCOPE_BOOK };
            let interest = new_interest.get();
            let comment = new_comment.get();
            // Atomic create + apply — no separate set_tag to race the create and drop
            // the chosen scope/interest.
            t.create_apply.dispatch(CreateAndApplyTag {
                name: raw,
                comment: comment.clone(),
                scope: scope.into(),
                interest: interest.clone(),
                kind: new_kind.get_untracked(),
                scale_max: new_max.get_untracked(),
                book_id,
                word_id,
            });
            // optimistic: register the tag + its application locally.
            t.tags.update(|v| {
                if !v.iter().any(|d| d.name == clean) {
                    v.push(TagDef {
                        name: clean.clone(),
                        comment: (!comment.trim().is_empty()).then(|| comment.clone()),
                        builtin: false,
                        scope: scope.into(),
                        interest,
                        sort: 999,
                        section: String::new(),
                        kind: new_kind.get_untracked(),
                        scale_max: if new_kind.get_untracked() == "scale" { new_max.get_untracked() } else { 1 },
                        scale_labels: None,
                    });
                }
            });
            t.store.update(|m| {
                m.entry(key).or_default().insert(clean.clone(), 1);
            });
        }
        toast_tag(toast, t.tags, &clean);
        new_name.set(String::new());
        new_comment.set(String::new());
        new_interest.set("interesting".to_string());
        new_word.set(false);
        new_kind.set("bool".to_string());
        new_max.set(5);
        expand.set(false);
    };

    // One scope group (book or word): section subheadings + tappable chips. In "edit"
    // mode chips gain pointer-drag; in "descriptions" mode each expands to show its
    // editable comment.
    let group_view = move |is_word: bool| {
        let describing = describe.get();
        let scoped: Vec<TagDef> = t.tags.get().into_iter()
            .filter(|d| d.name != "star" && (d.scope == SCOPE_WORD) == is_word)
            .collect();
        section_runs(scoped).into_iter().map(move |(section, items)| {
            let has_heading = !section.is_empty();
            let head_sect = section.clone();
            view! {
                {has_heading.then(|| {
                    let lbl = head_sect.clone();
                    view! { <div class="tagsection" attr:data-section=head_sect.clone()>{lbl}</div> }
                })}
                {items.into_iter().map(|d| {
                    let name = d.name.clone();
                    let on_name = name.clone();
                    let lbl_name = name.clone();
                    let tap_name = name.clone();
                    let click_name = name.clone();
                    let a_name = name.clone();
                    let imp_name = name.clone();
                    let over_name = name.clone();
                    let drag_name = name.clone();
                    let desc_name = name.clone();
                    // dedicated clones for the rate strip (the chip's own closures
                    // consume on_name / click_name / name, so the strip can't reuse them).
                    let (r_clr_s, r_clr_c, r_no_s, r_no_c, r_lv) =
                        (name.clone(), name.clone(), name.clone(), name.clone(), name.clone());
                    let title = d.comment.clone().unwrap_or_default();
                    let is_scale = d.is_scale();
                    let maxlv = d.max_level();
                    let cls = format!("chip int-{}", d.interest);
                    let comment_val = d.comment.clone().unwrap_or_default();
                    // Pointer-drag engages only in edit mode; below the drag
                    // threshold a press is a tap → apply (normal) or select (edit).
                    let (down, mv, up) = drag_handlers(
                        ds, t.tags, t.layout, editing, is_word, drag_name,
                        move || toast_tag(toast, t.tags, &tap_name),
                    );
                    let chip = view! {
                        <button type="button" class=cls title=title
                            attr:data-tagname=name.clone()
                            class:editing=move || editing.get()
                            class:scale=is_scale
                            class:on=move || has_tag(t, key, &on_name)
                            // "implied": lit only because an applied child implies this parent.
                            class:implied=move || !has_tag(t, key, &imp_name) && implied_on(t, key, &imp_name)
                            class:dragging=move || ds.active.get().as_deref() == Some(a_name.as_str())
                            class:dropover=move || matches!(ds.over.get(), Some((ref n, false)) if *n == over_name)
                            on:pointerdown=down on:pointermove=mv on:pointerup=up
                            on:pointercancel=move |_| ds.reset()
                            on:click=move |_| if !editing.get_untracked() {
                                // bool: toggle applied/untagged. scale: quick on at 1,
                                // or off — the rate strip sets a precise level.
                                if is_scale {
                                    let cur = tag_value(t, key, &click_name);
                                    set_tag_val(t, book_id, word_id, &click_name,
                                        if cur.is_some_and(|v| v >= 1) { None } else { Some(1) });
                                } else {
                                    toggle_tag(t, book_id, word_id, &click_name);
                                }
                                toast_tag(toast, t.tags, &click_name);
                            }>
                            <span class="chiptext">{name.clone()}</span>
                            // scale value badge (reactive), shown when applied.
                            {is_scale.then(|| view! {
                                <span class="chipval">{move || match tag_value(t, key, &lbl_name) {
                                    Some(v) if v >= 1 => format!(" ·{v}"),
                                    _ => String::new(),
                                }}</span>
                            })}
                        </button>
                    };
                    if describing {
                        view! {
                            <div class="chipcell">
                                {chip}
                                <input class="chipdesc" placeholder="describe this tag…" prop:value=comment_val
                                    on:change=move |ev| {
                                        let c = event_target_value(&ev);
                                        t.add.dispatch(AddTag { name: desc_name.clone(), comment: c.clone() });
                                        t.tags.update(|v| { if let Some(dd) = v.iter_mut().find(|dd| dd.name == desc_name) {
                                            dd.comment = (!c.trim().is_empty()).then(|| c.clone()); } });
                                    }/>
                            </div>
                        }.into_any()
                    } else if rating.get() {
                        // Rate mode: a tri-state / scale strip under every tag.
                        // ⌫ = untag (never considered), ✗ = considered & declined (0),
                        // 1..max = applied level (a bool tag has just "1" = yes).
                        view! {
                            <div class="chipcell">
                                {chip}
                                <div class="ratebar">
                                    <button type="button" class="ratebtn clr" title="untag"
                                        class:sel=move || tag_value(t, key, &r_clr_s).is_none()
                                        on:click=move |_| set_tag_val(t, book_id, word_id, &r_clr_c, None)>"⌫"</button>
                                    <button type="button" class="ratebtn no" title="considered — not tagged"
                                        class:sel=move || tag_value(t, key, &r_no_s) == Some(0)
                                        on:click=move |_| set_tag_val(t, book_id, word_id, &r_no_c, Some(0))>"✗"</button>
                                    {(1..=maxlv).map(|lv| {
                                        let nml = r_lv.clone();
                                        let ncl = r_lv.clone();
                                        view! { <button type="button" class="ratebtn"
                                            class:sel=move || tag_value(t, key, &nml) == Some(lv)
                                            on:click=move |_| set_tag_val(t, book_id, word_id, &ncl, Some(lv))>
                                            {if is_scale { lv.to_string() } else { "✓".to_string() }}</button> }
                                    }).collect_view()}
                                </div>
                            </div>
                        }.into_any()
                    } else {
                        chip.into_any()
                    }
                }).collect_view()}
            }
        }).collect_view()
    };

    view! {
        <div class="picker tagpick">
            <div class="pickhead">
                <button type="button" class="descbtn" class:on=move || describe.get()
                    title="show / edit each tag's description"
                    on:click=move |_| describe.update(|d| *d = !*d)>"ⓘ descriptions"</button>
                <button type="button" class="descbtn" class:on=move || rating.get()
                    title="rate each tag: untag / considered (✗) / a 1–N level"
                    on:click=move |_| rating.update(|r| *r = !*r)>"◐ rate"</button>
                <button type="button" class="descbtn" class:on=move || editing.get()
                    title="drag chips to reorder or refile them (touch-friendly)"
                    on:click=move |_| editing.update(|e| *e = !*e)>"↕ edit"</button>
            </div>
            <div class="scopegrp">
                <span class="picklbl">"this book"</span>
                {move || group_view(false)}
            </div>
            <div class="scopegrp global">
                <span class="picklbl" title="word-scoped: applies to this word in every book">"all books"</span>
                {move || group_view(true)}
            </div>
            {has_buckets.then(|| view! {
                <div class="pickgroup">
                    <span class="picklbl">"good for: "</span>
                    {buckets.into_iter().map(|b| {
                        let tag = format!("pick:{b}");
                        let on_tag = tag.clone();
                        let click_tag = tag.clone();
                        view! {
                            <button type="button" class="chip pick"
                                class:on=move || has_tag(t, key, &on_tag)
                                on:click=move |_| toggle_tag(t, book_id, word_id, &click_tag)>{b}</button>
                        }
                    }).collect_view()}
                </div>
            })}
            <div class="pickgroup newtag">
                <input class="newtag-name" placeholder="search or + new tag"
                    prop:value=move || new_name.get()
                    on:input=move |ev| { new_name.set(event_target_value(&ev)); expand.set(false); }
                    on:keydown=move |ev| if ev.key() == "Enter" { add_new(); }/>
                // Live search over the existing collection: tap a match to apply it;
                // if nothing matches the typed name, offer to create it (which reveals
                // the scope/interest form).
                {move || {
                    let q = new_name.get();
                    let ql = q.trim().to_lowercase();
                    if ql.is_empty() {
                        return ().into_any();
                    }
                    let clean = sanitize_tag(&q);
                    let matches: Vec<TagDef> = t.tags.get().into_iter()
                        .filter(|d| d.name != "star" && d.name.to_lowercase().contains(&ql))
                        .take(10)
                        .collect();
                    let exact = clean.as_ref().is_some_and(|c| t.tags.with(|v| v.iter().any(|d| &d.name == c)));
                    view! {
                        <div class="tagsearch">
                            {matches.into_iter().map(|d| {
                                let nm = d.name.clone();
                                let on_nm = nm.clone();
                                let click_nm = nm.clone();
                                let cls = format!("chip int-{}", d.interest);
                                view! {
                                    <button type="button" class=cls class:on=move || has_tag(t, key, &on_nm)
                                        title=d.comment.clone().unwrap_or_default()
                                        on:click=move |_| {
                                            if !has_tag(t, key, &click_nm) { toggle_tag(t, book_id, word_id, &click_nm); }
                                            toast_tag(toast, t.tags, &click_nm);
                                            new_name.set(String::new());
                                        }>
                                        {d.name.clone()}
                                    </button>
                                }
                            }).collect_view()}
                            {(!exact && clean.is_some()).then(|| view! {
                                <button type="button" class="chip add" class:on=move || expand.get()
                                    on:click=move |_| expand.update(|e| *e = !*e)>
                                    {move || format!("＋ create \"{}\"", new_name.get())}
                                </button>
                            })}
                        </div>
                    }.into_any()
                }}
                // The create form (comment + scope + interest), hidden until "create …".
                {move || expand.get().then(|| view! {
                    <div class="newtag-opts">
                        <input class="newtag-comment" placeholder="what it's for (optional)"
                            prop:value=move || new_comment.get()
                            on:input=move |ev| new_comment.set(event_target_value(&ev))
                            on:keydown=move |ev| if ev.key() == "Enter" { add_new(); }/>
                        <select class="newtag-sel" title="scope" prop:value=move || if new_word.get() { "word" } else { "book" }
                            on:change=move |ev| new_word.set(event_target_value(&ev) == "word")>
                            <option value="book">"this book"</option>
                            <option value="word">"all books"</option>
                        </select>
                        <select class="newtag-sel" title="interest" prop:value=move || new_interest.get()
                            on:change=move |ev| new_interest.set(event_target_value(&ev))>
                            <option value="interesting">"favourite"</option>
                            <option value="neutral">"note"</option>
                            <option value="uninteresting">"negative"</option>
                        </select>
                        <select class="newtag-sel" title="a scale lets you rate 1–N instead of on/off"
                            prop:value=move || new_kind.get()
                            on:change=move |ev| new_kind.set(event_target_value(&ev))>
                            <option value="bool">"on / off"</option>
                            <option value="scale">"scale"</option>
                        </select>
                        {move || (new_kind.get() == "scale").then(|| view! {
                            <input class="newtag-max" type="number" min="2" max="10" title="top of the scale"
                                prop:value=move || new_max.get().to_string()
                                on:input=move |ev| {
                                    if let Ok(n) = event_target_value(&ev).parse::<i64>() {
                                        new_max.set(n.clamp(2, 10));
                                    }
                                }/>
                        })}
                        <button type="button" class="chip add" on:click=move |_| add_new()>"add"</button>
                    </div>
                })}
                <a class="managelink" href=format!("{}/tags", base_path())>"manage ↗"</a>
            </div>
        </div>
    }
}

#[component]
fn Trajectory(data: Vec<(i32, f64)>, book_year: Option<i64>) -> impl IntoView {
    let w = 240.0_f64;
    let h = 56.0_f64;
    let plot_h = h - 12.0;
    if data.is_empty() {
        return view! { <p class="traj-empty">"no usage-over-time data"</p> }.into_any();
    }
    let max_pm = data.iter().map(|(_, p)| *p).fold(0.0_f64, f64::max).max(1e-9);
    let min_dec = *data.iter().map(|(d, _)| d).min().unwrap();
    let max_dec = *data.iter().map(|(d, _)| d).max().unwrap();
    let n = data.len() as f64;
    let bar_w = (w / n) * 0.78;
    let bars: Vec<(f64, f64, f64)> = data.iter().enumerate().map(|(i, (_, pm))| {
        let x = (i as f64) * (w / n) + (w / n - bar_w) / 2.0;
        let bh = (pm / max_pm) * plot_h;
        (x, plot_h - bh, bh)
    }).collect();
    let marker = book_year.map(|y| {
        let bdec = (y as f64 / 10.0).floor() * 10.0;
        let span = ((max_dec - min_dec) as f64).max(10.0);
        let raw = (bdec - min_dec as f64) / span;
        let frac = raw.clamp(0.0, 1.0);
        // A book past the charted data pins to the right edge; label it at the TOP
        // so its year doesn't collide with the bottom-right axis decade label.
        (frac * (w - (w / n)) + (w / n) / 2.0, y, raw > 1.0)
    });
    view! {
        <svg class="traj" width=w height=h viewBox=format!("0 0 {w} {h}") role="img" aria-label="usage over time">
            {bars.into_iter().map(|(x, y, bh)| view! {
                <rect x=x y=y width=bar_w height=bh class="traj-bar"/>
            }).collect_view()}
            {marker.map(|(mx, yr, beyond)| view! {
                <line x1=mx y1=0.0 x2=mx y2=plot_h class="traj-marker"/>
                <text x=mx y={if beyond { 8.0 } else { h }} text-anchor="middle" class="traj-yr">{yr.to_string()}</text>
            })}
            <text x=0.0 y=h class="traj-ax">{format!("{min_dec}s")}</text>
            <text x=w y=h text-anchor="end" class="traj-ax">{format!("{max_dec}s")}</text>
        </svg>
    }.into_any()
}

#[component]
fn HomePage() -> impl IntoView {
    let (book_q, set_book) = query_signal::<i64>("book");
    let (category, set_cat) = query_signal::<String>("cat");
    // scroll:false so opening the detail sidebar (or jumping to a related word)
    // doesn't scroll the list back to the top.
    let (selected, set_word) =
        query_signal_with_options::<i64>("word", NavigateOptions { scroll: false, ..Default::default() });
    // stemming aggressiveness: 0 none / 1 inflectional / 2 derivational / 3 aggressive
    let (lvl_q, set_lvl) = query_signal::<i64>("lvl");
    let level = Memo::new(move |_| lvl_q.get().unwrap_or(0));
    // Load the book list up front so the current-book pick can fall back to a real
    // book when the requested one is missing (e.g. it was just deleted).
    let books = Resource::new(|| (), |_| list_books());
    // No explicit ?book → resume the last-viewed book (localStorage), else the first.
    // If that id isn't among the existing books (stale localStorage, or a deleted
    // book still named in the URL or the nav "words" link), fall back to the first
    // available book — otherwise every per-book query returns empty and the page
    // strands on a wordless, tagless "0 shown" view of a book that no longer exists.
    let book = Memo::new(move |_| {
        let want = book_q.get().or_else(stored_book).unwrap_or(1);
        match books.get() {
            Some(Ok(list)) if !list.is_empty() && !list.iter().any(|b| b.id == want) => list[0].id,
            _ => want,
        }
    });
    let only_top = RwSignal::new(false);
    let hide_rejected = RwSignal::new(false);
    let open_picker = RwSignal::new(None::<i64>);

    let tagger = Tagger {
        store: RwSignal::new(HashMap::new()),
        action: ServerAction::<SetTag>::new(),
        set_val: ServerAction::<SetTagValue>::new(),
        tags: RwSignal::new(Vec::new()),
        add: ServerAction::<AddTag>::new(),
        create: ServerAction::<CreateTag>::new(),
        create_apply: ServerAction::<CreateAndApplyTag>::new(),
        scope: ServerAction::<SetTagScope>::new(),
        interest: ServerAction::<SetTagInterest>::new(),
        set_scale: ServerAction::<SetTagScale>::new(),
        rename: ServerAction::<RenameTag>::new(),
        del: ServerAction::<DeleteTag>::new(),
        layout: ServerAction::<SetScopeLayout>::new(),
    };
    provide_context(tagger);

    // Keep the shared "current book" + its localStorage copy in sync, so the nav
    // "words" link and a bare `/` resume this book.
    let current = expect_context::<CurrentBook>();
    Effect::new(move |_| {
        let b = book.get();
        remember_book(b);
        current.0.set(Some(b));
    });

    // The tag collection (builtin + custom), refetched after any tag-collection edit.
    let tag_defs = Resource::new(move || tagger_rev(tagger), |_| list_tags());
    Effect::new(move |_| {
        if let Some(Ok(defs)) = tag_defs.get() {
            tagger.tags.set(defs);
        }
    });

    let categories = Resource::new(move || (book.get(), level.get()), |(b, l)| list_categories(b, l));
    let candidates = Resource::new(
        move || (book.get(), category.get(), level.get()),
        move |(b, cat, l)| get_candidates(b, cat, 400, l),
    );
    let detail = Resource::new(
        move || (book.get(), selected.get(), level.get()),
        move |(b, sel, l)| async move {
            match sel {
                Some(wid) => word_detail(b, wid, l).await.map(Some),
                None => Ok(None),
            }
        },
    );

    // Seed the client tag store with EVERY tagged word in the book (not just the
    // candidate representatives), so the family union reads correctly at any level.
    // Refetch when the book changes or when an edit alters applications (scope move /
    // rename / delete), and replace this book's entries so removals take effect.
    let all_tags = Resource::new(
        move || (book.get(), tagger_apps_rev(tagger)),
        |(b, _)| book_tags(b),
    );
    Effect::new(move |_| {
        if let Some(Ok(rows)) = all_tags.get() {
            let b = book.get();
            tagger.store.update(|m| {
                m.retain(|(bk, _), _| *bk != b);
                for (wid, tags) in rows {
                    m.insert((b, wid), tags.into_iter().collect());
                }
            });
        }
    });

    view! {
        <h1>"coolwords"</h1>
        <p class="sub">"★ to favourite; click \"tags\" to label; click a word for detail, a category to filter."</p>

        <div class="bar">
            <Suspense fallback=move || view! { <span>"…"</span> }>
                {move || books.get().map(|res| match res {
                    Err(e) => view! { <span class="err">{format!("{e}")}</span> }.into_any(),
                    Ok(list) => view! {
                        {list.into_iter().map(|b| {
                            let id = b.id;
                            view! {
                                <button class:active=move || book.get() == id
                                    on:click=move |_| { set_book.set(Some(id)); set_word.set(None); }>
                                    {format!("{} ({}★)", b.title, b.n_selected)}
                                </button>
                            }
                        }).collect_view()}
                    }.into_any(),
                })}
            </Suspense>
            <a class="srclink" href={let b = base_path(); move || format!("{b}/source?book={}", book.get())}
                title="see which parts of this book were kept vs stripped">"view stripping"</a>
            <select class="lvlsel" title="merge related word forms: none keeps every form separate; higher levels group inflections, then derivations, then aggressively (untrembling→tremble) — frequency is combined across the family"
                prop:value=move || level.get().to_string()
                on:change=move |ev| {
                    let v = event_target_value(&ev).parse::<i64>().unwrap_or(0);
                    set_lvl.set(if v == 0 { None } else { Some(v) });
                }>
                <option value="0">"merge: none"</option>
                <option value="1">"merge: inflections"</option>
                <option value="2">"merge: derivations"</option>
                <option value="3">"merge: aggressive"</option>
            </select>
            <Suspense fallback=|| ()>
                {move || categories.get().map(|res| match res {
                    Err(_) => ().into_any(),
                    Ok(opts) => {
                        // opts arrive in group order; fold consecutive runs into <optgroup>s.
                        let mut groups: Vec<(String, Vec<FilterOpt>)> = Vec::new();
                        for o in opts {
                            match groups.last_mut() {
                                Some(g) if g.0 == o.group => g.1.push(o),
                                _ => groups.push((o.group.clone(), vec![o])),
                            }
                        }
                        view! {
                            <select class="catsel"
                                prop:value=move || category.get().unwrap_or_default()
                                on:change=move |ev| {
                                    let v = event_target_value(&ev);
                                    if v.is_empty() { set_cat.set(None); } else { set_cat.set(Some(v)); }
                                }>
                                <option value="">"all categories"</option>
                                {groups.into_iter().map(|(g, items)| view! {
                                    <optgroup label=g>
                                        {items.into_iter().map(|o| view! {
                                            <option value=o.value.clone()>
                                                {format!("{} ({})", o.label, o.count)}
                                            </option>
                                        }).collect_view()}
                                    </optgroup>
                                }).collect_view()}
                            </select>
                        }.into_any()
                    }
                })}
            </Suspense>
            <Show when=move || category.get().is_some() fallback=|| ()>
                <button class="catx" title="clear category filter" on:click=move |_| set_cat.set(None)>"×"</button>
            </Show>
            <label class="toggle">
                <input type="checkbox" prop:checked=move || only_top.get()
                    on:change=move |_| only_top.update(|v| *v = !*v)/>
                " varied top-20 only"
            </label>
            <label class="toggle" title="hide words tagged with an 'uninteresting' tag (junk / proper-noun / not-a-word)">
                <input type="checkbox" prop:checked=move || hide_rejected.get()
                    on:change=move |_| hide_rejected.update(|v| *v = !*v)/>
                " hide rejected"
            </label>
        </div>

        <Suspense fallback=move || view! { <p class="loading">"Loading…"</p> }>
            {move || candidates.get().map(|res| match res {
                Err(e) => view! { <p class="err">{format!("Error: {e}")}</p> }.into_any(),
                Ok(all) => {
                    let b = book.get();
                    let top = only_top.get();
                    let list: Vec<Candidate> = all.into_iter().filter(|c| !top || c.selected).collect();
                    let total = list.len();
                    view! {
                        <p class="counts">{format!("{total} shown")}</p>
                        <div class="wlist">
                            {list.into_iter().map(|c| {
                                let wid = c.word_id;
                                let bk = c.buckets.clone();
                                let selected_dot = c.selected;
                                let nforms = c.n_forms;
                                let members = c.members.clone();
                                let members_rej = c.members.clone();
                                let members_hid = c.members.clone();
                                let members_has = c.members.clone();
                                let members_strip = c.members.clone();
                                let word = c.word.clone();
                                let gloss = c.gloss.clone().unwrap_or_default();
                                let has_gloss = !gloss.is_empty();
                                let in_book = c.in_book;
                                let score = c.score;
                                let origin_disp = c.origin_name.clone().or_else(|| c.origin_code.clone()).unwrap_or_default();
                                let has_origin = !origin_disp.is_empty();
                                let origin_title = c.origin_code.clone().unwrap_or_default();
                                let cat = c.category.clone();
                                view! {
                                    <article class="wcard"
                                        class:interesting=move || group_interest(tagger, b, &members) == Some(Interest::Interesting)
                                        class:rejected=move || group_interest(tagger, b, &members_rej) == Some(Interest::Uninteresting)
                                        class:hidden=move || hide_rejected.get() && group_interest(tagger, b, &members_hid) == Some(Interest::Uninteresting)>
                                        <div class="wc-main">
                                            <Star book_id=b word_id=wid/>
                                            <div class="wc-body" on:click=move |_| set_word.set(Some(wid))>
                                                <div class="wc-head">
                                                    <span class="word">
                                                        {word}
                                                        {(nforms > 1).then(|| view! {
                                                            <small class="forms" title="surface forms merged into this group at the current level">{format!(" +{}", nforms - 1)}</small>
                                                        })}
                                                    </span>
                                                    {selected_dot.then(|| view! { <span class="seldot" title="in the varied top-20">"•"</span> })}
                                                </div>
                                                {has_gloss.then(|| view! { <p class="gloss">{gloss}</p> })}
                                                <div class="wc-meta">
                                                    <span title="times in this book">{format!("{in_book}×")}</span>
                                                    <span title="interestingness score">{format!("score {:.1}", score)}</span>
                                                    {has_origin.then(|| view! { <span class="wc-origin" title=origin_title>{origin_disp}</span> })}
                                                </div>
                                            </div>
                                            <div class="wc-side">
                                                {cat.map(|cc| { let cc2 = cc.clone(); view! {
                                                    <button type="button" class="catchip" title="filter by this category"
                                                        on:click=move |_| set_cat.set(Some(cc2.clone()))>{cc}</button>
                                                } })}
                                            </div>
                                        </div>
                                        <div class="wc-tags">
                                            <TagStrip book_id=b members=members_strip/>
                                            <button type="button" class="tagbtn"
                                                class:has=move || group_has_other(tagger, b, &members_has)
                                                class:open=move || open_picker.get() == Some(wid)
                                                on:click=move |_| open_picker.update(|o| *o = if *o == Some(wid) { None } else { Some(wid) })>
                                                {move || if open_picker.get() == Some(wid) { "− tag" } else { "＋ tag" }}
                                            </button>
                                        </div>
                                        <Show when=move || open_picker.get() == Some(wid) fallback=|| ()>
                                            <TagPicker book_id=b word_id=wid buckets=bk.clone()/>
                                        </Show>
                                    </article>
                                }
                            }).collect_view()}
                        </div>
                    }.into_any()
                }
            })}
        </Suspense>

        <Show when=move || selected.get().is_some() fallback=|| ()>
            <div class="detail-backdrop" on:click=move |_| set_word.set(None)></div>
            <aside class="detail">
                <button class="close" title="back to the list" on:click=move |_| set_word.set(None)>"← back"</button>
                <Suspense fallback=move || view! { <p class="loading">"…"</p> }>
                    {move || detail.get().map(|res| match res {
                        Err(e) => view! { <p class="err">{format!("{e}")}</p> }.into_any(),
                        Ok(None) => ().into_any(),
                        Ok(Some(d)) => {
                            let b = book.get();
                            let wid = d.word_id;
                            let origin = d.origin_name.clone().or_else(|| d.origin_code.clone()).unwrap_or_default();
                            let mut groups: Vec<(String, Vec<RelTarget>)> = Vec::new();
                            for rt in d.relations.clone() {
                                if let Some(last) = groups.last_mut() {
                                    if last.0 == rt.rel { last.1.push(rt); continue; }
                                }
                                groups.push((rt.rel.clone(), vec![rt]));
                            }
                            view! {
                                <h2>{d.word.clone()}</h2>
                                <div class="detail-tags">
                                    <Star book_id=b word_id=wid/>
                                    <TagPicker book_id=b word_id=wid buckets=d.buckets.clone()/>
                                </div>
                                <p class="gloss">{d.gloss.clone().unwrap_or_default()}</p>
                                {d.example.clone().map(|ex| {
                                    let segs = highlight(&ex, &d.word);
                                    view! { <blockquote class="ex">
                                        {segs.into_iter().map(|(s, hit)| if hit {
                                            view! { <strong>{s}</strong> }.into_any()
                                        } else { view! { {s} }.into_any() }).collect_view()}
                                    </blockquote> }
                                })}
                                <ul class="meta">
                                    <li>{format!("in this book: {}×", d.in_book)}</li>
                                    <li>{format!("frequency: {:.3}/M", d.freq_pm.unwrap_or(0.0))}</li>
                                    <li>{format!("syllables: {}", d.syllables.map(|n| n.to_string()).unwrap_or_default())}</li>
                                    <li>{format!("origin: {origin}")}</li>
                                </ul>
                                <Trajectory data=d.trajectory.clone() book_year=d.book_year/>
                                {(d.era.is_some() || d.obsolete).then(|| {
                                    let era = d.era.clone();
                                    let obs = d.obsolete;
                                    view! {
                                        <p class="era">
                                            {era.map(|e| view! {
                                                <span title="trajectory relative to this book's decade — ahead of its time: rare then, common later; of its time: peaked around then; declining: its heyday was earlier; timeless: roughly steady; always rare: never common in any era">
                                                    "usage: " <strong>{e}</strong>
                                                </span>
                                            })}
                                            {obs.then(|| view! {
                                                <span class="badge-obs" title="had real usage once, but is effectively extinct today — measured against the present day, not the book's era">"obsolete today"</span>
                                            })}
                                        </p>
                                    }
                                })}
                                {d.root.clone().map(|r| {
                                    let rid = r.word_id;
                                    let rword = r.word.clone();
                                    let cat = r.category.clone().map(|c| format!(" · {c}")).unwrap_or_default();
                                    view! {
                                        <p class="caps rootlbl">
                                            "root word: "
                                            <a class="reltgt" on:click=move |_| set_word.set(Some(rid))>{rword}</a>
                                            {format!("  {:.1}/M{cat}", r.freq_pm.unwrap_or(0.0))}
                                        </p>
                                        <Trajectory data=r.trajectory.clone() book_year=d.book_year/>
                                    }
                                })}
                                <Show when={let c = d.categories.clone(); move || !c.is_empty()} fallback=|| ()>
                                    <p class="caps">"categories: "
                                        {d.categories.clone().into_iter().map(|cat| {
                                            let cc = cat.clone();
                                            view! { <button class="catchip" on:click=move |_| set_cat.set(Some(cc.clone()))>{cat}</button> }
                                        }).collect_view()}
                                    </p>
                                </Show>
                                <Show when={let bb = d.base.clone(); move || bb.is_some()} fallback=|| ()>
                                    <p class="base">
                                        {let bb = d.base.clone().unwrap();
                                         format!("likely a variant of a more common word: {} ({:.1}/M)", bb.0, bb.1)}
                                    </p>
                                </Show>
                                // Show the merged forms whenever there's more than one, OR the single
                                // in-book form differs from the headword (the lemma isn't itself in the
                                // book) — so a merged entry like "sev" isn't left looking orphaned.
                                <Show when={let f = d.family.clone(); let hw = d.word.clone();
                                            move || f.len() > 1 || f.iter().any(|m| m.1 != hw)} fallback=|| ()>
                                    <p class="caps">"forms merged here (★ to tag a variant):"</p>
                                    <ul class="family">
                                        {d.family.clone().into_iter().map(|(fwid, fw, n, fp)| view! {
                                            <li>
                                                <Star book_id=b word_id=fwid/>
                                                <span class="word" on:click=move |_| set_word.set(Some(fwid))>{fw}</span>
                                                {format!(" — {n}× here, {fp:.1}/M overall")}
                                            </li>
                                        }).collect_view()}
                                    </ul>
                                </Show>
                                <Show when={let r = groups.clone(); move || !r.is_empty()} fallback=|| ()>
                                    <p class="caps">"WordNet relations (bold = also in this book):"</p>
                                    <ul class="rels">
                                        {groups.clone().into_iter().map(|(rel, items)| view! {
                                            <li>
                                                <span class="rel">{format!("{rel}: ")}</span>
                                                {items.into_iter().enumerate().map(|(i, rt)| {
                                                    let sep = if i > 0 { ", " } else { "" };
                                                    let cls = if rt.in_book { "reltgt inbook" } else { "reltgt" };
                                                    let target = rt.target.clone();
                                                    match rt.target_word_id {
                                                        Some(id) => view! {
                                                            <span>{sep}<a class=cls on:click=move |_| set_word.set(Some(id))>{target}</a></span>
                                                        }.into_any(),
                                                        None => view! { <span>{sep}<span class=cls>{target}</span></span> }.into_any(),
                                                    }
                                                }).collect_view()}
                                            </li>
                                        }).collect_view()}
                                    </ul>
                                </Show>
                                <Show when={let a = d.also_in.clone(); move || !a.is_empty()} fallback=|| ()>
                                    <p class="caps">"also in — the same word in other books (word-scoped tags reach all of these):"</p>
                                    <p class="alsoin">
                                        {let wid = d.word_id;
                                         d.also_in.clone().into_iter().map(move |(bid, title, n)| view! {
                                            <a class="reltgt" title="open this word in that book"
                                                on:click=move |_| { set_book.set(Some(bid)); set_word.set(Some(wid)); }>
                                                {title}</a>
                                            <small class="alsoin-n">{format!(" ({n}) ")}</small>
                                        }).collect_view()}
                                    </p>
                                </Show>
                            }.into_any()
                        }
                    })}
                </Suspense>
            </aside>
        </Show>
    }
}

// ---- tag editor (/tags) ----

/// One editable row in the tag manager: drag to reorder, rename, comment, section,
/// scope, interest, delete. `all`/`layout` back the drag-reorder + section edits.
#[component]
fn TagRow(
    d: TagDef,
    all: RwSignal<Vec<TagDef>>,
    layout: ServerAction<SetScopeLayout>,
    ds: DragState,
    editing: RwSignal<bool>,
    rename: ServerAction<RenameTag>,
    del: ServerAction<DeleteTag>,
    interest: ServerAction<SetTagInterest>,
    scope: ServerAction<SetTagScope>,
    set_scale: ServerAction<SetTagScale>,
    comment_act: ServerAction<AddTag>,
) -> impl IntoView {
    let toast = expect_context::<Toast>();
    let name = d.name.clone();
    let locked = name == "star";
    let is_word = d.scope == SCOPE_WORD;
    let cur_interest = d.interest.clone();
    let int_cls = d.interest.clone();
    let comment = d.comment.clone().unwrap_or_default();
    let section = d.section.clone();
    // dotted-hierarchy depth (`thing.material` sits one level under `thing`) → indent.
    let depth = ancestor_names(&name).len();
    let is_child = depth > 0;
    let cur_kind = if d.is_scale() { "scale" } else { "bool" };
    let cur_max = d.max_level() as i64;
    let (n_scale, n_scale2) = (name.clone(), name.clone());
    let (n_ren, n_com, n_int, n_scope, n_del, n_sec, grip_name, tap_name) = (
        name.clone(), name.clone(), name.clone(), name.clone(), name.clone(), name.clone(), name.clone(), name.clone(),
    );
    let (down, mv, up) = drag_handlers(
        ds, all, layout, editing, is_word, grip_name,
        move || toast_tag(toast, all, &tap_name),
    );

    // A scope change re-homes (book→word) or drops (word→book) a tag's applications
    // and generally can't be undone, so warn in BOTH directions once more than one
    // word is involved. Empty / single-application tags switch silently.
    let toggle_scope = move |_| {
        let nm = n_scope.clone();
        let to_word = !is_word;
        leptos::task::spawn_local(async move {
            let cnt = tag_usage(nm.clone()).await.unwrap_or(0);
            let msg = if to_word {
                format!("Make '{nm}' apply to this word across ALL books? It's applied to {cnt} word(s) now; this re-homes those applications and usually can't be undone.")
            } else {
                format!("Make '{nm}' book-only again? It's applied to {cnt} word(s) across all books; this drops those applications and usually can't be undone.")
            };
            let ok = cnt <= 1
                || web_sys::window()
                    .and_then(|w| w.confirm_with_message(&msg).ok())
                    .unwrap_or(false);
            if ok {
                let new_scope = if to_word { SCOPE_WORD } else { SCOPE_BOOK };
                scope.dispatch(SetTagScope { name: nm, scope: new_scope.into(), convert_book: 0 });
            }
        });
    };
    let do_delete = move |_| {
        let nm = n_del.clone();
        let ok = web_sys::window()
            .and_then(|w| w.confirm_with_message(&format!("Delete tag '{nm}' and remove it from all tagged words?")).ok())
            .unwrap_or(false);
        if ok {
            del.dispatch(DeleteTag { name: nm });
        }
    };

    view! {
        <tr class=format!("tagrow int-{int_cls}") attr:data-tagname=name.clone()
            class:editing=move || editing.get()>
            <td class="grip" class:draghandle=move || editing.get()
                title="turn on “edit”, then drag this handle to reorder / refile"
                on:pointerdown=down on:pointermove=mv on:pointerup=up
                on:pointercancel=move |_| ds.reset()>"⠿"</td>
            <td>
                {if locked {
                    view! { <span class="tagname locked" title="the quick ★ tag can't be renamed or deleted">{name.clone()}" ★"</span> }.into_any()
                } else {
                    view! { <input class="tagname" class:child=is_child
                        style=format!("margin-left:{}em", depth as f64 * 1.1)
                        title=if is_child { format!("child of '{}'", ancestor_names(&name).last().cloned().unwrap_or_default()) } else { String::new() }
                        prop:value=name.clone()
                        on:change=move |ev| { let v = event_target_value(&ev);
                            if let Some(clean) = sanitize_tag(&v) {
                                if clean != n_ren {
                                    rename.dispatch(RenameTag { old: n_ren.clone(), new: v });
                                    // optimistic: relabel this row AND its dotted descendants
                                    // (the server cascades them) so their rows' keys update and
                                    // later edits don't target now-dead names and get silently dropped.
                                    all.update(|list| { for dd in list.iter_mut() {
                                        if dd.name == n_ren {
                                            dd.name = clean.clone();
                                        } else if is_ancestor(&n_ren, &dd.name) {
                                            dd.name = format!("{clean}{}", &dd.name[n_ren.len()..]);
                                        }
                                    } });
                                }
                            } }/> }.into_any()
                }}
            </td>
            <td>
                <input class="tagcomment" prop:value=comment placeholder="what it's for"
                    on:change=move |ev| { let c = event_target_value(&ev);
                        comment_act.dispatch(AddTag { name: n_com.clone(), comment: c.clone() });
                        all.update(|list| { if let Some(dd) = list.iter_mut().find(|dd| dd.name == n_com) {
                            dd.comment = (!c.trim().is_empty()).then(|| c.clone()); } }); }/>
            </td>
            <td>
                <input class="tagsectionin" list="tagsections" placeholder="—" prop:value=section
                    title="subheading within this scope (type a new name to create a section)"
                    on:change=move |ev| set_section(all, layout, n_sec.clone(), is_word, event_target_value(&ev))/>
            </td>
            <td>
                <button class="scopebtn" class:global=is_word title="toggle book / word scope" on:click=toggle_scope>
                    {if is_word { "word — all books" } else { "book" }}
                </button>
            </td>
            <td>
                <select class="intsel" prop:value=cur_interest disabled=locked
                    on:change=move |ev| { let iv = event_target_value(&ev);
                        interest.dispatch(SetTagInterest { name: n_int.clone(), interest: iv.clone() });
                        all.update(|list| { if let Some(dd) = list.iter_mut().find(|dd| dd.name == n_int) { dd.interest = iv.clone(); } }); }>
                    <option value="interesting">"interesting · favourites"</option>
                    <option value="neutral">"neutral · note"</option>
                    <option value="uninteresting">"uninteresting · junk"</option>
                </select>
            </td>
            <td class="scalecell">
                {(!locked).then(|| {
                    let kmax = n_scale.clone();
                    let ksel = n_scale2.clone();
                    view! {
                        <select class="kindsel" prop:value=cur_kind title="on/off tag, or a 1–N scale"
                            on:change=move |ev| { let k = event_target_value(&ev);
                                let mx = if k == "scale" { 5 } else { 1 };
                                set_scale.dispatch(SetTagScale { name: ksel.clone(), kind: k.clone(), scale_max: mx });
                                all.update(|list| { if let Some(dd) = list.iter_mut().find(|dd| dd.name == ksel) {
                                    dd.kind = k.clone(); dd.scale_max = mx; } }); }>
                            <option value="bool">"on / off"</option>
                            <option value="scale">"scale"</option>
                        </select>
                        {move || all.with(|l| l.iter().find(|dd| dd.name == kmax).is_some_and(|dd| dd.is_scale())).then(|| {
                            let kmx = kmax.clone();   // for the reactive value read
                            let kmc = kmax.clone();   // for the change handler
                            view! { <input class="maxin" type="number" min="2" max="10" title="top of the scale"
                                prop:value=move || all.with(|l| l.iter().find(|dd| dd.name == kmx).map(|dd| dd.scale_max).unwrap_or(cur_max)).to_string()
                                on:change=move |ev| { if let Ok(n) = event_target_value(&ev).parse::<i64>() {
                                    let n = n.clamp(2, 10);
                                    set_scale.dispatch(SetTagScale { name: kmc.clone(), kind: "scale".into(), scale_max: n });
                                    all.update(|list| { if let Some(dd) = list.iter_mut().find(|dd| dd.name == kmc) { dd.scale_max = n; } });
                                } }/> }
                        })}
                    }
                })}
            </td>
            <td>
                {(!locked).then(|| view! { <button class="catx" title="delete tag" on:click=do_delete>"✕"</button> })}
            </td>
        </tr>
    }
}

/// One rendered line in a scope group: a section subheading or a tag row. Keyed
/// (`key`) so `<For>` preserves each row's DOM node across optimistic edits — inputs
/// keep focus and the page keeps its scroll position instead of remounting.
#[derive(Clone)]
enum RowItem {
    Heading(String),
    Tag(TagDef),
}

impl RowItem {
    fn key(&self) -> String {
        match self {
            RowItem::Heading(s) => format!("h\u{1}{s}"),
            RowItem::Tag(d) => format!("t\u{1}{}", d.name),
        }
    }
}

/// The tag manager page: turn on "edit" then drag the grip to reorder / refile;
/// rename, comment, assign sections, re-scope, set interest, sort, delete — grouped
/// into book-scoped and word-scoped tags, each subdivided by the user's sections.
#[component]
fn TagsPage() -> impl IntoView {
    let add = ServerAction::<AddTag>::new();
    let create = ServerAction::<CreateTag>::new();
    let rename = ServerAction::<RenameTag>::new();
    let del = ServerAction::<DeleteTag>::new();
    let interest = ServerAction::<SetTagInterest>::new();
    let scope = ServerAction::<SetTagScope>::new();
    let set_scale = ServerAction::<SetTagScale>::new();
    let layout = ServerAction::<SetScopeLayout>::new();
    // Only *structural* edits (add a row, delete a row, move a row between the
    // book/word groups) refetch the list. Rename / comment / interest / reorder /
    // section are applied optimistically to `all` below, so they don't refetch —
    // refetching would remount the whole table and jump the scroll to the top after
    // every keystroke. A server-side failure of any optimistic edit triggers one
    // reconciling refetch via the effect further down.
    let rev = move || {
        create.version().get() + del.version().get() + scope.version().get()
    };
    let tags = Resource::new(rev, |_| list_tags());
    // Mirror the fetched collection into a signal we can reorder optimistically.
    let all = RwSignal::new(Vec::<TagDef>::new());
    Effect::new(move |_| {
        if let Some(Ok(list)) = tags.get() {
            all.set(list);
        }
    });
    // Reconcile `all` with the DB if any optimistic edit was rejected server-side.
    Effect::new(move |_| {
        let failed = matches!(rename.value().get(), Some(Err(_)))
            || matches!(interest.value().get(), Some(Err(_)))
            || matches!(layout.value().get(), Some(Err(_)))
            || matches!(set_scale.value().get(), Some(Err(_)))
            || matches!(add.value().get(), Some(Err(_)));
        if failed {
            tags.refetch();
        }
    });
    let ds = DragState::new();
    // "edit" mode gates drag-to-reorder so a stray swipe on mobile can't reshuffle
    // tags; off by default (rows are plain, inputs stay tappable).
    let editing = RwSignal::new(false);

    let new_name = RwSignal::new(String::new());
    let new_comment = RwSignal::new(String::new());
    let new_word = RwSignal::new(false);
    let new_interest = RwSignal::new("interesting".to_string());
    let new_kind = RwSignal::new("bool".to_string());
    let new_max = RwSignal::new(5i64);
    let add_new = move || {
        let raw = new_name.get();
        let Some(clean) = sanitize_tag(&raw) else { return };
        let scope_s = if new_word.get() { SCOPE_WORD } else { SCOPE_BOOK };
        let interest_s = new_interest.get();
        let comment = new_comment.get();
        let kind_s = new_kind.get();
        let max_v = if kind_s == "scale" { new_max.get() } else { 1 };
        create.dispatch(CreateTag {
            name: raw, comment: comment.clone(), scope: scope_s.into(), interest: interest_s.clone(),
            kind: kind_s.clone(), scale_max: max_v,
        });
        all.update(|v| {
            if !v.iter().any(|d| d.name == clean) {
                v.push(TagDef {
                    name: clean.clone(),
                    comment: (!comment.trim().is_empty()).then(|| comment.clone()),
                    builtin: false,
                    scope: scope_s.into(),
                    interest: interest_s,
                    sort: 999,
                    section: String::new(),
                    kind: kind_s,
                    scale_max: max_v,
                    scale_labels: None,
                });
            }
        });
        new_name.set(String::new());
        new_comment.set(String::new());
        new_kind.set("bool".to_string());
        new_max.set(5);
    };

    // Rows for one scope, as a keyed list (section headings + tag rows). `<For>`
    // keeps each row's DOM node stable across optimistic edits, so reorders move
    // nodes rather than remounting the whole table (no scroll jump / focus loss).
    let scope_rows = move |is_word: bool| {
        view! {
            <For
                each=move || {
                    let scoped: Vec<TagDef> = all.get().into_iter()
                        .filter(|d| (d.scope == SCOPE_WORD) == is_word).collect();
                    section_runs(scoped).into_iter().flat_map(|(section, items)| {
                        let head = (!section.is_empty()).then(|| RowItem::Heading(section.clone()));
                        head.into_iter().chain(items.into_iter().map(RowItem::Tag)).collect::<Vec<_>>()
                    }).collect::<Vec<_>>()
                }
                key=|item: &RowItem| item.key()
                let:item
            >
                {match item {
                    RowItem::Heading(s) => view! {
                        <tr class="sectionhdr" attr:data-section=s.clone()>
                            <td></td><td colspan="7">{s.clone()}</td>
                        </tr>
                    }.into_any(),
                    RowItem::Tag(d) => view! {
                        <TagRow d=d all=all layout=layout ds=ds editing=editing rename=rename del=del
                            interest=interest scope=scope set_scale=set_scale comment_act=add/>
                    }.into_any(),
                }}
            </For>
        }
    };
    // Sort controls for one scope — sort within each section run, persisted as a layout.
    let sort_btns = move |is_word: bool| view! {
        <span class="sortbtns">
            <button type="button" class="sortbtn" title="sort each section A→Z"
                on:click=move |_| sort_scope(all, layout, is_word, false)>"A–Z"</button>
            <button type="button" class="sortbtn" title="sort each section by interest, then A→Z"
                on:click=move |_| sort_scope(all, layout, is_word, true)>"by interest"</button>
        </span>
    };

    view! {
        <h1>"tags"</h1>
        <p class="sub">"Turn on “edit” to drag the ⠿ handle and reorder or refile tags; type a section name to file a tag under it. Interesting tags favourite a word; uninteresting tags mark it as junk (hideable)."</p>
        <div class="pickhead tagsedit">
            <button type="button" class="descbtn" class:on=move || editing.get()
                title="drag the ⠿ handle to reorder / refile tags"
                on:click=move |_| editing.update(|e| *e = !*e)>"↕ edit / reorder"</button>
        </div>
        <datalist id="tagsections">
            {move || {
                let mut secs: Vec<String> = all.get().into_iter().map(|d| d.section).filter(|s| !s.is_empty()).collect();
                secs.sort(); secs.dedup();
                secs.into_iter().map(|s| view! { <option value=s></option> }).collect_view()
            }}
        </datalist>
        <Suspense fallback=move || view! { <p class="loading">"Loading…"</p> }>
            {move || tags.get().map(|res| match res {
                Err(e) => view! { <p class="err">{format!("{e}")}</p> }.into_any(),
                Ok(_) => view! {
                    <table class="tagtable">
                        <thead><tr>
                            <th></th><th>"tag"</th><th>"comment / what it's for"</th>
                            <th>"section"</th><th>"scope"</th><th>"interest"</th><th>"scale"</th><th></th>
                        </tr></thead>
                        <tbody>
                            <tr class="grouphdr"><td colspan="8">"book tags — a word in one book "{move || sort_btns(false)}</td></tr>
                            {move || scope_rows(false)}
                            <tr class="dropend"><td colspan="8">"— end of book tags (drop here to move to the end) —"</td></tr>
                            <tr class="grouphdr"><td colspan="8">"word tags — a word across all books "{move || sort_btns(true)}</td></tr>
                            {move || scope_rows(true)}
                            <tr class="dropend"><td colspan="8">"— end of word tags (drop here to move to the end) —"</td></tr>
                        </tbody>
                    </table>
                }.into_any(),
            })}
        </Suspense>
        <div class="pickgroup newtag tagsadd">
            <input class="newtag-name" placeholder="+ new tag" prop:value=move || new_name.get()
                on:input=move |ev| new_name.set(event_target_value(&ev))
                on:keydown=move |ev| if ev.key() == "Enter" { add_new(); }/>
            <input class="newtag-comment" placeholder="what it's for (optional)" prop:value=move || new_comment.get()
                on:input=move |ev| new_comment.set(event_target_value(&ev))
                on:keydown=move |ev| if ev.key() == "Enter" { add_new(); }/>
            <select class="newtag-sel" title="scope" prop:value=move || if new_word.get() { "word" } else { "book" }
                on:change=move |ev| new_word.set(event_target_value(&ev) == "word")>
                <option value="book">"this book"</option>
                <option value="word">"all books"</option>
            </select>
            <select class="newtag-sel" title="interest" prop:value=move || new_interest.get()
                on:change=move |ev| new_interest.set(event_target_value(&ev))>
                <option value="interesting">"favourite"</option>
                <option value="neutral">"note"</option>
                <option value="uninteresting">"negative"</option>
            </select>
            <select class="newtag-sel" title="on/off tag, or a 1–N scale" prop:value=move || new_kind.get()
                on:change=move |ev| new_kind.set(event_target_value(&ev))>
                <option value="bool">"on / off"</option>
                <option value="scale">"scale"</option>
            </select>
            {move || (new_kind.get() == "scale").then(|| view! {
                <input class="newtag-max" type="number" min="2" max="10" title="top of the scale"
                    prop:value=move || new_max.get().to_string()
                    on:input=move |ev| { if let Ok(n) = event_target_value(&ev).parse::<i64>() { new_max.set(n.clamp(2, 10)); } }/>
            })}
            <button class="chip add" on:click=move |_| add_new()>"add tag"</button>
        </div>
    }
}

// ---- collection: tagged words across all books ----

/// A cross-book view of every word you've tagged, filterable to one tag. Each word
/// links back into a book's detail; word-scoped tags list all the books they reach.
#[component]
fn CollectionPage() -> impl IntoView {
    let (tag_q, set_tag) = query_signal::<String>("tag");
    let (sort_q, set_sort) = query_signal::<String>("sort");
    let base = base_path();
    let tags = Resource::new(|| (), |_| collection_tags());
    let words = Resource::new(move || tag_q.get(), collection_words);

    view! {
        <h1>"collection"</h1>
        <p class="sub">"every word you've tagged, across all books. Filter by a tag; click a word to open it in a book."</p>

        <div class="bar">
            <Suspense fallback=|| ()>
                {move || tags.get().map(|res| match res {
                    Err(_) => ().into_any(),
                    Ok(list) => view! {
                        <select class="catsel"
                            prop:value=move || tag_q.get().unwrap_or_default()
                            on:change=move |ev| {
                                let v = event_target_value(&ev);
                                if v.is_empty() { set_tag.set(None); } else { set_tag.set(Some(v)); }
                            }>
                            <option value="">"all tags"</option>
                            <option value="special:top-global">"★ most all-books favourites"</option>
                            <optgroup label="tags">
                                {list.into_iter().map(|(name, n)| { let v = name.clone(); view! {
                                    <option value=v>{format!("{name} ({n})")}</option>
                                } }).collect_view()}
                            </optgroup>
                        </select>
                    }.into_any(),
                })}
            </Suspense>
            <select class="catsel" title="sort"
                prop:value=move || sort_q.get().unwrap_or_default()
                on:change=move |ev| {
                    let v = event_target_value(&ev);
                    if v.is_empty() { set_sort.set(None); } else { set_sort.set(Some(v)); }
                }>
                <option value="">"sort: default"</option>
                <option value="interest">"sort: favourites first"</option>
                <option value="az">"sort: A–Z"</option>
                <option value="tags">"sort: most tags"</option>
                <option value="books">"sort: most books"</option>
            </select>
            <Show when=move || tag_q.get().is_some() fallback=|| ()>
                <button class="catx" title="clear filter" on:click=move |_| set_tag.set(None)>"×"</button>
            </Show>
        </div>

        <Suspense fallback=move || view! { <p class="loading">"Loading…"</p> }>
            {move || words.get().map(|res| match res {
                Err(e) => view! { <p class="err">{format!("Error: {e}")}</p> }.into_any(),
                Ok(mut list) => {
                    if list.is_empty() {
                        return view! { <p class="sub">"no tagged words here yet — favourite (★) or tag words on the words page."</p> }.into_any();
                    }
                    // client-side re-sort (server returns interest- or metric-ordered by default).
                    let int_rank = |i: &str| match i { "interesting" => 0, "neutral" => 1, _ => 2 };
                    match sort_q.get().as_deref() {
                        Some("az") => list.sort_by(|a, b| a.word.cmp(&b.word)),
                        Some("tags") => list.sort_by(|a, b| b.tags.len().cmp(&a.tags.len()).then_with(|| a.word.cmp(&b.word))),
                        Some("books") => list.sort_by(|a, b| b.books.len().cmp(&a.books.len()).then_with(|| a.word.cmp(&b.word))),
                        Some("interest") => list.sort_by(|a, b| int_rank(&a.interest).cmp(&int_rank(&b.interest)).then_with(|| a.word.cmp(&b.word))),
                        _ => {} // keep server order
                    }
                    let base = base.clone();
                    view! {
                        <p class="counts">{format!("{} words", list.len())}</p>
                        <div class="wlist">
                            {list.into_iter().map(|e| {
                                let word_href = (e.word_id > 0).then(|| {
                                    e.books.first().map(|(bid, _)| format!("{base}/?book={bid}&word={}", e.word_id))
                                }).flatten();
                                let gloss = short(&e.gloss, 140);
                                let has_gloss = !gloss.is_empty();
                                let cls = format!("wcard ccard int-{}", e.interest);
                                let word = e.word.clone();
                                let metric = e.metric;
                                view! {
                                    <article class=cls>
                                        <div class="wc-body">
                                            {metric.map(|m| view! { <span class="cc-metric" title="net all-books favourites (favourite − negative)">{format!("+{m}")}</span> })}
                                            {match word_href {
                                                Some(h) => view! { <a class="word" href=h>{word}</a> }.into_any(),
                                                None => view! { <span class="word">{word}</span> }.into_any(),
                                            }}
                                            {has_gloss.then(|| view! { <p class="gloss">{gloss}</p> })}
                                        </div>
                                        <div class="wc-tags">
                                            <span class="tagstrip">
                                                {e.tags.into_iter().map(|(n, i)| {
                                                    let cls = format!("taglabel int-{i}");
                                                    view! { <span class=cls>{n}</span> }
                                                }).collect_view()}
                                            </span>
                                        </div>
                                        // Only when the word still lives in a book. A word whose
                                        // sole book was deleted keeps its tags but has nowhere to
                                        // link, so drop the empty "in:" line rather than show it bare.
                                        {(!e.books.is_empty()).then(|| view! {
                                            <div class="cc-books">
                                                <span class="cc-in">"in: "</span>
                                                {e.books.into_iter().map(|(bid, title)| {
                                                    let href = (e.word_id > 0)
                                                        .then(|| format!("{base}/?book={bid}&word={}", e.word_id));
                                                    match href {
                                                        Some(h) => view! { <a class="cc-book" href=h>{title}</a> }.into_any(),
                                                        None => view! { <span class="cc-book">{title}</span> }.into_any(),
                                                    }
                                                }).collect_view()}
                                            </div>
                                        })}
                                    </article>
                                }
                            }).collect_view()}
                        </div>
                    }.into_any()
                }
            })}
        </Suspense>
    }
}

// ---- book import UI ----

/// The drop/inspect lifecycle for the import page.
#[derive(Clone)]
enum UploadState {
    Idle,
    Loading,
    Done(Box<Inspection>),
    Failed(String),
}

/// Send a dropped file to `upload_book` and stash the resulting inspection in
/// `state`. The FormData / `.into()` codec only exists on the client, so the body
/// is client-only; the SSR stub never runs (event handlers fire in the browser).
#[cfg(not(feature = "ssr"))]
fn upload_and_inspect(file: web_sys::File, state: RwSignal<UploadState>) {
    state.set(UploadState::Loading);
    let fd = web_sys::FormData::new().unwrap();
    let _ = fd.append_with_blob_and_filename("file", &file, &file.name());
    leptos::task::spawn_local(async move {
        match upload_book(fd.into()).await {
            Ok(insp) => state.set(UploadState::Done(Box::new(insp))),
            Err(e) => state.set(UploadState::Failed(e.to_string())),
        }
    });
}
#[cfg(feature = "ssr")]
fn upload_and_inspect(_file: web_sys::File, _state: RwSignal<UploadState>) {}

/// Render a kept-vs-stripped segmentation (shared by the import preview and the
/// per-book "view stripping" page). Kept body spans render plain; stripped spans
/// (Gutenberg header/licence, TOC, EPUB front-matter, ...) are dimmed and labelled.
#[component]
fn SegmentView(segments: Vec<ImportSegment>) -> impl IntoView {
    let kept: i64 = segments.iter().filter(|s| s.kept).map(|s| s.char_len).sum();
    let stripped: i64 = segments.iter().filter(|s| !s.kept).map(|s| s.char_len).sum();
    view! {
        <p class="seg-summary">
            {format!("{} regions · kept {} chars · stripped {} chars", segments.len(), kept, stripped)}
        </p>
        <div class="segs">
            {segments.into_iter().map(|s| {
                let badge = if s.kept { "kept".to_string() } else { s.label.clone() };
                let text = if s.truncated { format!("{} …", s.preview) } else { s.preview.clone() };
                view! {
                    <div class="seg" class:kept=s.kept class:stripped=!s.kept>
                        <div class="seg-head">
                            <span class="seg-badge">{badge}</span>
                            <span class="seg-note">{s.note}</span>
                            <span class="seg-len">{format!("{} chars", s.char_len)}</span>
                        </div>
                        <pre class="seg-text">{text}</pre>
                    </div>
                }
            }).collect_view()}
        </div>
    }
}

/// The OCR-compare lifecycle for a PDF.
#[derive(Clone)]
enum CompareState {
    Idle,
    Loading,
    Done(Box<OcrCompareResult>),
    Failed(String),
}

/// Per-page embedded-vs-OCR diff panel: similarity badge + an inline word diff
/// (red strikethrough = embedded-only, green = OCR-only, grey = shared context).
#[component]
fn OcrCompareView(cmp: OcrCompareResult) -> impl IntoView {
    view! {
        <div class="ocr-compare">
            <p class="seg-summary">
                {format!("OCR engine: {} · {} pages sampled — red = embedded only, green = OCR only",
                    cmp.engine, cmp.pages.len())}
            </p>
            {cmp.pages.into_iter().map(|p| {
                let pct = (p.sim * 100.0).round() as i64;
                let simcls = format!("ocr-sim {}", if pct >= 90 { "hi" } else if pct >= 70 { "mid" } else { "lo" });
                view! {
                    <div class="ocr-page">
                        <div class="ocr-phead">
                            <strong>{format!("page {}", p.page)}</strong>
                            <span class=simcls>{format!("{pct}% match")}</span>
                            <span class="seg-len">{format!("{}w embedded · {}w OCR", p.embedded_words, p.ocr_words)}</span>
                        </div>
                        <p class="ocr-diff">
                            {p.ops.into_iter().map(|o| match o.op.as_str() {
                                "gap" => view! { <span class="d-gap">{format!(" {} ", o.a)}</span> }.into_any(),
                                "del" => view! { <span class="d-del">{format!("{} ", o.a)}</span> }.into_any(),
                                "ins" => view! { <span class="d-ins">{format!("{} ", o.b)}</span> }.into_any(),
                                "rep" => view! {
                                    <span class="d-del">{format!("{} ", o.a)}</span>
                                    <span class="d-ins">{format!("{} ", o.b)}</span>
                                }.into_any(),
                                _ => view! { <span class="d-eq">{format!("{} ", o.a)}</span> }.into_any(),
                            }).collect_view()}
                        </p>
                    </div>
                }
            }).collect_view()}
        </div>
    }
}

/// Drag-drop a `.txt`/`.epub`/`.pdf`, review detected metadata + what gets stripped
/// (and for PDFs, compare embedded text vs re-OCR), then commit it.
#[component]
fn ImportPage() -> impl IntoView {
    let state = RwSignal::new(UploadState::Idle);
    let f_title = RwSignal::new(String::new());
    let f_author = RwSignal::new(String::new());
    let f_year = RwSignal::new(String::new());
    let f_slug = RwSignal::new(String::new());
    let committing = RwSignal::new(false);
    let commit_err = RwSignal::new(None::<String>);
    let file_input: NodeRef<leptos::html::Input> = NodeRef::new();

    // Seed the editable form from each fresh inspection.
    Effect::new(move |_| {
        if let UploadState::Done(insp) = state.get() {
            f_title.set(insp.title.clone());
            f_author.set(insp.author.clone());
            f_year.set(insp.year.map(|y| y.to_string()).unwrap_or_default());
            f_slug.set(insp.suggested_slug.clone());
        }
    });

    let do_upload = Callback::new(move |file: web_sys::File| upload_and_inspect(file, state));
    let on_drop = move |ev: web_sys::DragEvent| {
        ev.prevent_default();
        if let Some(file) = ev.data_transfer().and_then(|dt| dt.files()).and_then(|f| f.get(0)) {
            do_upload.run(file);
        }
    };
    let on_dragover = move |ev: web_sys::DragEvent| ev.prevent_default();
    let on_pick = move |_ev| {
        if let Some(file) = file_input.get().and_then(|i| i.files()).and_then(|f| f.get(0)) {
            do_upload.run(file);
        }
    };

    // Import is always fast (embedded). A scanned PDF imports as a placeholder and
    // we auto-start a background OCR job, then send the user to the manage page.
    let do_commit = move |_| {
        let UploadState::Done(insp) = state.get() else { return };
        committing.set(true);
        commit_err.set(None);
        let token = insp.token.clone();
        let orig = insp.orig_filename.clone();
        let (title, author, year, slug) = (f_title.get(), f_author.get(), f_year.get(), f_slug.get());
        let needs_ocr = insp.needs_ocr;
        let engine = insp.ocr.as_ref().map(|o| o.default_engine.clone()).unwrap_or_default();
        let navigate = use_navigate();
        let base = base_path();
        leptos::task::spawn_local(async move {
            match confirm_import(token, slug, title, author, year, orig).await {
                Ok(res) => {
                    if needs_ocr && !engine.is_empty() {
                        let _ = start_ocr(res.book_id, engine).await; // fire-and-forget
                        navigate(&format!("{base}/books"), Default::default());
                    } else {
                        navigate(&format!("{base}/?book={}", res.book_id), Default::default());
                    }
                }
                Err(e) => {
                    commit_err.set(Some(e.to_string()));
                    committing.set(false);
                }
            }
        });
    };

    view! {
        <h1>"Import a book"</h1>
        <p class="sub">"Drop a .txt, .epub or .pdf; we detect title/author and show what gets stripped. "
            "PDFs can be re-OCR'd and compared to the embedded text."</p>

        <div class="dropzone" on:drop=on_drop on:dragover=on_dragover>
            <p class="dz-big">"Drag a .txt, .epub or .pdf here"</p>
            <p>"or "<label class="dz-browse">"browse…"
                <input type="file" accept=".txt,.epub,.pdf" node_ref=file_input
                    on:change=on_pick style="display:none"/>
            </label></p>
        </div>

        {move || match state.get() {
            UploadState::Idle => ().into_any(),
            UploadState::Loading => view! { <p class="loading">"Reading & analysing…"</p> }.into_any(),
            UploadState::Failed(e) => view! { <p class="err">{e}</p> }.into_any(),
            UploadState::Done(insp) => {
                let fmt = insp.format.clone();
                let year_note = insp.year_note.clone();
                let ntok = insp.n_tokens;
                let ntypes = insp.n_types;
                let is_dup = insp.duplicate_of.is_some();
                let dup_banner = insp.duplicate_of.clone().map(|slug| {
                    let title = insp.duplicate_title.clone().unwrap_or_else(|| slug.clone());
                    view! {
                        <div class="dup-banner">
                            {format!("⚠ Identical content already imported as \"{title}\" ({slug}). Re-importing is blocked.")}
                        </div>
                    }
                });
                let segs = insp.segments.clone();
                let pdf = insp.pdf.clone();
                let needs_ocr = insp.needs_ocr;
                view! {
                    {dup_banner}
                    <div class="meta-form">
                        <label>"Title"
                            <input prop:value=move || f_title.get()
                                on:input=move |e| f_title.set(event_target_value(&e))/></label>
                        <label>"Author"
                            <input prop:value=move || f_author.get()
                                on:input=move |e| f_author.set(event_target_value(&e))/></label>
                        <label class="yr">"Year"
                            <input prop:value=move || f_year.get()
                                on:input=move |e| f_year.set(event_target_value(&e))/></label>
                        <label>"Slug"
                            <input prop:value=move || f_slug.get()
                                on:input=move |e| f_slug.set(event_target_value(&e))/></label>
                    </div>
                    {(!year_note.is_empty()).then(|| view! { <p class="yr-note">{year_note}</p> })}
                    <p class="counts">{format!("format: {fmt} · {ntok} tokens · {ntypes} distinct types")}</p>
                    {pdf.map(|p| {
                        let stats = format!("PDF · {} pages — {} with text, {} image-only",
                            p.n_pages, p.n_text_pages, p.n_image_pages);
                        view! {
                            <p class="counts">{stats}</p>
                            {needs_ocr.then(|| view! {
                                <div class="dup-banner ocr-banner">
                                    "No embedded text layer — this looks like a scan. It imports now as a "
                                    "placeholder and OCR runs automatically in the background; manage it on "
                                    <A href=format!("{}/books", base_path())>"the books page"</A>"."
                                </div>
                            })}
                        }
                    })}
                    <div class="detail-actions">
                        <button class="commit" disabled=move || committing.get() || is_dup
                            on:click=do_commit>
                            {move || if committing.get() {
                                view! {
                                    <span class="busy"><span class="spinner"></span>
                                        "Importing… (scoring may take a moment)"</span>
                                }.into_any()
                            } else {
                                view! { <span>"Confirm import"</span> }.into_any()
                            }}
                        </button>
                    </div>
                    {move || commit_err.get().map(|e| view! { <p class="err">{e}</p> })}
                    <h2 class="seg-h">"Kept vs stripped"</h2>
                    <SegmentView segments=segs.clone()/>
                }.into_any()
            }
        }}
    }
}

/// Per-book "view stripping" page: re-segment a stored imported file so the kept /
/// stripped regions can be reviewed after the fact.
#[component]
fn BookSourcePage() -> impl IntoView {
    let (book_q, _) = query_signal::<i64>("book");
    let book = Memo::new(move |_| book_q.get().unwrap_or(1));
    let src = Resource::new(move || book.get(), view_source);
    view! {
        <h1>"Book source — kept vs stripped"</h1>
        <p class="sub">"Kept regions are analysed; stripped boilerplate (license, TOC, front-matter) is ignored."</p>
        <Suspense fallback=move || view! { <p class="loading">"Loading…"</p> }>
            {move || src.get().map(|res| match res {
                Err(e) => view! { <p class="err">{e.to_string()}</p> }.into_any(),
                Ok(insp) => view! {
                    <p class="counts">
                        {format!("{} · {} · {} tokens", insp.title, insp.format, insp.n_tokens)}
                    </p>
                    <SegmentView segments=insp.segments/>
                }.into_any(),
            })}
        </Suspense>
    }
}

// ---- book management page (/books) ---- #

/// Polls a background job (~800 ms) and renders a progress bar + Cancel while it's
/// live. Clears `job` and fires `on_done` when it finishes. Reused for OCR /
/// reingest / trajectory jobs.
#[component]
fn JobProgressBar(job: RwSignal<Option<String>>, #[prop(optional)] on_done: Option<Callback<()>>) -> impl IntoView {
    let prog = RwSignal::new(None::<JobProgress>);
    Effect::new(move |_| {
        // client-only (effects don't run on SSR); one poller for this bar's lifetime.
        let h = leptos::prelude::set_interval_with_handle(
            move || {
                if let Some(id) = job.get_untracked() {
                    leptos::task::spawn_local(async move {
                        if let Ok(p) = job_status(id).await {
                            let live = matches!(p.as_ref().map(|x| x.status.as_str()),
                                Some("queued") | Some("running"));
                            prog.set(p);
                            if !live {
                                job.set(None);
                                if let Some(cb) = on_done {
                                    cb.run(());
                                }
                            }
                        }
                    });
                }
            },
            std::time::Duration::from_millis(800),
        );
        if let Ok(h) = h {
            on_cleanup(move || h.clear());
        }
    });
    let cancel = move |_| {
        if let Some(id) = job.get() {
            leptos::task::spawn_local(async move { let _ = cancel_job(id).await; });
        }
    };
    view! {
        {move || prog.get().filter(|p| matches!(p.status.as_str(), "queued" | "running")).map(|p| {
            let pct = p.percent;
            view! {
                <div class="jobbar">
                    <span class="busy"><span class="spinner"></span>{p.message.clone()}</span>
                    {(pct >= 0.0).then(|| view! { <progress class="jobprog" max="100" value=pct></progress> })}
                    <button type="button" class="chip" on:click=cancel>"Cancel"</button>
                </div>
            }
        })}
    }
}

/// PDF OCR + text-source controls for one book: per-engine cache state, Run OCR
/// (background) / Compare / Delete cache, and a source selector that re-ingests.
#[component]
fn OcrPanel(book_id: i64) -> impl IntoView {
    let refresh = RwSignal::new(0u32);
    let status = Resource::new(move || (book_id, refresh.get()), move |(b, _)| book_ocr_status(b));
    let job = RwSignal::new(None::<String>);
    let compare = RwSignal::new(CompareState::Idle);
    let sel_source = RwSignal::new(String::new());

    // seed the source selector to the book's current source, once.
    Effect::new(move |_| {
        if let Some(Ok(s)) = status.get() {
            if sel_source.get_untracked().is_empty() {
                sel_source.set(s.text_source.unwrap_or_else(|| "embedded".into()));
            }
        }
    });

    let run_ocr = move |engine: String| {
        leptos::task::spawn_local(async move {
            if let Ok(id) = start_ocr(book_id, engine).await {
                job.set(Some(id));
            }
        });
    };
    let do_compare = move |engine: String| {
        compare.set(CompareState::Loading);
        leptos::task::spawn_local(async move {
            match ocr_compare_book(book_id, engine).await {
                Ok(c) => compare.set(CompareState::Done(Box::new(c))),
                Err(e) => compare.set(CompareState::Failed(e.to_string())),
            }
        });
    };
    let del_cache = move |engine: String| {
        leptos::task::spawn_local(async move {
            let _ = delete_ocr(book_id, engine).await;
            refresh.update(|n| *n += 1);
        });
    };
    let apply_source = move |_| {
        let src = sel_source.get();
        if src.is_empty() {
            return;
        }
        leptos::task::spawn_local(async move {
            if let Ok(id) = start_reingest(book_id, src).await {
                job.set(Some(id));
            }
        });
    };

    view! {
        <div class="ocrpanel">
            <Suspense fallback=move || view! { <span class="loading">"…"</span> }>
                {move || status.get().map(|res| match res {
                    Err(e) => view! { <p class="err">{e.to_string()}</p> }.into_any(),
                    Ok(s) if !s.is_pdf => ().into_any(),
                    Ok(s) => {
                        let cur = s.text_source.clone().unwrap_or_else(|| "embedded".into());
                        let npages = s.n_pages;
                        let mut engs: Vec<(String, OcrEngineStatus)> = s.engines.into_iter().collect();
                        engs.sort_by(|a, b| a.0.cmp(&b.0));
                        let mut sources = vec!["embedded".to_string()];
                        for (n, e) in &engs {
                            if e.complete {
                                sources.push(format!("ocr:{n}"));
                            }
                        }
                        view! {
                            <p class="ocr-cur">{format!("current source: {cur} · {npages} pages")}</p>
                            <ul class="ocr-engs">
                                {engs.into_iter().map(|(name, e)| {
                                    if !e.available {
                                        return view! { <li class="ocr-eng off">{format!("{name}: not installed")}</li> }.into_any();
                                    }
                                    let (n1, n2, n3) = (name.clone(), name.clone(), name.clone());
                                    let cache_lbl = if e.complete { format!("{}/{} ✓", e.cached_pages, npages) }
                                        else if e.cached_pages > 0 { format!("{}/{} partial", e.cached_pages, npages) }
                                        else { "not cached".to_string() };
                                    view! {
                                        <li class="ocr-eng">
                                            <span class="eng-name">{name}</span>
                                            <span class="eng-cache">{cache_lbl}</span>
                                            <button type="button" class="chip" on:click=move |_| run_ocr(n1.clone())>"Run OCR"</button>
                                            <button type="button" class="chip" on:click=move |_| do_compare(n2.clone())>"Compare"</button>
                                            {(e.cached_pages > 0).then(|| view! {
                                                <button type="button" class="chip" on:click=move |_| del_cache(n3.clone())>"Delete cache"</button>
                                            })}
                                        </li>
                                    }.into_any()
                                }).collect_view()}
                            </ul>
                            <div class="ocr-source">
                                <span class="picklbl">"use text from: "</span>
                                <select class="intsel" prop:value=move || sel_source.get()
                                    on:change=move |e| sel_source.set(event_target_value(&e))>
                                    {sources.into_iter().map(|s| { let v = s.clone(); view! { <option value=v>{s}</option> } }).collect_view()}
                                </select>
                                <button type="button" class="chip" on:click=apply_source>"Apply"</button>
                            </div>
                        }.into_any()
                    }
                })}
            </Suspense>
            <JobProgressBar job=job on_done=Callback::new(move |()| refresh.update(|n| *n += 1))/>
            {move || match compare.get() {
                CompareState::Idle => ().into_any(),
                CompareState::Loading => view! { <p class="loading">"OCR-ing sample pages…"</p> }.into_any(),
                CompareState::Failed(e) => view! { <p class="err">{e}</p> }.into_any(),
                CompareState::Done(c) => view! { <OcrCompareView cmp=*c/> }.into_any(),
            }}
        </div>
    }
}

/// One editable book row on the manage page.
#[component]
fn BookCard(b: BookAdmin, edit: ServerAction<UpdateBook>, del: ServerAction<DeleteBook>) -> impl IntoView {
    let BookAdmin {
        id, slug, title: t0, author: a0, year: y0, format, source, text_source,
        n_tokens, n_types, n_selected, ingested_at,
    } = b;
    let title = RwSignal::new(t0);
    let author = RwSignal::new(a0);
    let year = RwSignal::new(y0.map(|y| y.to_string()).unwrap_or_default());
    let is_pdf = format == "pdf";
    let save = move || edit.dispatch(UpdateBook {
        book_id: id, title: title.get(), author: author.get(), year: year.get(),
    });
    let del_slug = slug.clone();
    let do_delete = move |_| {
        let msg = format!("Delete '{del_slug}' and its analysis data? (your tags are kept and will return if you re-import.)");
        if web_sys::window().and_then(|w| w.confirm_with_message(&msg).ok()).unwrap_or(false) {
            del.dispatch(DeleteBook { book_id: id });
        }
    };
    let src = if text_source.is_empty() { source } else { text_source };
    view! {
        <div class="bookcard">
            <div class="bk-fields">
                <input class="bk-title" prop:value=move || title.get()
                    on:input=move |e| title.set(event_target_value(&e)) on:change=move |_| { save(); }/>
                <input class="bk-author" placeholder="author" prop:value=move || author.get()
                    on:input=move |e| author.set(event_target_value(&e)) on:change=move |_| { save(); }/>
                <input class="bk-year" placeholder="year" prop:value=move || year.get()
                    on:input=move |e| year.set(event_target_value(&e)) on:change=move |_| { save(); }/>
                <button type="button" class="catx" title="delete book" on:click=do_delete>"✕"</button>
            </div>
            <p class="bk-meta">
                <a class="reltgt" href=format!("{}/?book={id}", base_path())>{slug}</a>
                {format!(" · {format} · {src} · {n_tokens} tokens · {n_types} types · {n_selected}★ · {ingested_at}")}
            </p>
            {is_pdf.then(|| view! { <OcrPanel book_id=id/> })}
        </div>
    }
}

/// The book management page: edit details, delete, and (for PDFs) manage OCR +
/// switch text source — all heavy work runs as background jobs.
#[component]
fn BooksAdminPage() -> impl IntoView {
    let edit = ServerAction::<UpdateBook>::new();
    let del = ServerAction::<DeleteBook>::new();
    let books = Resource::new(move || del.version().get(), |_| list_books_admin());
    let traj_job = RwSignal::new(None::<String>);
    let do_traj = move |_| {
        leptos::task::spawn_local(async move {
            if let Ok(id) = refresh_trajectory().await {
                traj_job.set(Some(id));
            }
        });
    };
    view! {
        <h1>"books"</h1>
        <p class="sub">"Edit details, manage PDF OCR, switch text source."</p>
        <div class="bar">
            <button type="button" class="chip" on:click=do_traj>"refresh usage charts (all books)"</button>
            <JobProgressBar job=traj_job/>
        </div>
        <Suspense fallback=move || view! { <p class="loading">"Loading…"</p> }>
            {move || books.get().map(|res| match res {
                Err(e) => view! { <p class="err">{e.to_string()}</p> }.into_any(),
                Ok(list) => view! {
                    <div class="booklist">
                        {list.into_iter().map(|b| view! { <BookCard b=b edit=edit del=del/> }).collect_view()}
                    </div>
                }.into_any(),
            })}
        </Suspense>
    }
}
