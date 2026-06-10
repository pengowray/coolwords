use std::collections::{HashMap, HashSet};
#[cfg(feature = "ssr")]
use std::collections::BTreeSet;

use leptos::prelude::*;
use leptos_meta::{provide_meta_context, MetaTags, Stylesheet, Title};
use leptos_router::components::{Route, Router, Routes, A};
use leptos_router::hooks::{query_signal, query_signal_with_options, use_navigate};
use leptos_router::{NavigateOptions, StaticSegment};
use serde::{Deserialize, Serialize};

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
}

/// Normalize a free-text tag name to its canonical collection form, or None if
/// it isn't a usable tag. Pure (client + server) so optimistic UI matches storage.
pub fn sanitize_tag(name: &str) -> Option<String> {
    let kept: String = name
        .trim()
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == ' ' || *c == '-')
        .collect();
    let s = kept.split_whitespace().collect::<Vec<_>>().join(" ");
    if (1..=30).contains(&s.chars().count())
        && s.chars().any(|c| c.is_ascii_alphabetic())
        && !s.starts_with("pick:")
    {
        Some(s)
    } else {
        None
    }
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
    pub store: RwSignal<HashMap<(i64, i64), HashSet<String>>>,
    pub action: ServerAction<SetTag>,
    pub tags: RwSignal<Vec<TagDef>>,
    pub add: ServerAction<AddTag>,
    // tag-collection mutations (editor + drag); the picker/editor refetch the
    // collection (and book tags) whenever any of their versions change.
    pub scope: ServerAction<SetTagScope>,
    pub interest: ServerAction<SetTagInterest>,
    pub rename: ServerAction<RenameTag>,
    pub del: ServerAction<DeleteTag>,
    pub reorder: ServerAction<ReorderTag>,
    /// Tags moved/added to global *this session* with no prior global history — the
    /// global→local "just added, moved straight back ⇒ no warning" exception.
    pub just_global: RwSignal<HashSet<String>>,
}

/// Combined version of every tag-collection mutation — Resources watch this to
/// refetch `list_tags` / `book_tags` after any edit.
fn tagger_rev(t: Tagger) -> usize {
    t.add.version().get()
        + t.scope.version().get()
        + t.interest.version().get()
        + t.rename.version().get()
        + t.del.version().get()
        + t.reorder.version().get()
}

/// Version of just the mutations that change *applications* (word_tags rows), so
/// the per-book tag store refetches on scope move / rename / delete but not on a
/// plain add (whose application arrives via its own optimistic SetTag).
fn tagger_apps_rev(t: Tagger) -> usize {
    t.scope.version().get() + t.rename.version().get() + t.del.version().get()
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

/// A word's effective interest from its applied tags: interesting wins over
/// uninteresting wins over neutral; None if it carries no tags.
fn word_interest(t: Tagger, key: (i64, i64)) -> Option<Interest> {
    t.store.with(|m| {
        let set = m.get(&key)?;
        if set.is_empty() {
            return None;
        }
        let mut acc = Interest::Neutral;
        for tag in set {
            match tag_interest(t, tag) {
                Interest::Interesting => return Some(Interest::Interesting),
                Interest::Uninteresting => acc = Interest::Uninteresting,
                Interest::Neutral => {}
            }
        }
        Some(acc)
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

pub fn shell(options: LeptosOptions) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8"/>
                <meta name="viewport" content="width=device-width, initial-scale=1"/>
                <AutoReload options=options.clone() />
                <HydrationScripts options/>
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
    let mut have = HashSet::new();
    {
        let mut s = u.prepare("PRAGMA table_info(tags)").map_err(|e| ServerFnError::new(e.to_string()))?;
        let rows = s.query_map([], |r| r.get::<_, String>(1)).map_err(|e| ServerFnError::new(e.to_string()))?;
        for r in rows.filter_map(Result::ok) {
            have.insert(r);
        }
    }
    for (name, decl) in [("scope", "TEXT NOT NULL DEFAULT 'book'"),
                         ("interest", "TEXT NOT NULL DEFAULT 'interesting'")] {
        if !have.contains(name) {
            u.execute(&format!("ALTER TABLE tags ADD COLUMN {name} {decl}"), [])
                .map_err(|e| ServerFnError::new(e.to_string()))?;
        }
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
fn repo_root() -> String {
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
fn books_dir() -> std::path::PathBuf {
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
fn python_exe() -> String {
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
fn load_tags(conn: &rusqlite::Connection, book_id: i64) -> Result<HashMap<i64, Vec<String>>, ServerFnError> {
    use rusqlite::OptionalExtension;
    let slug: Option<String> = conn
        .query_row("SELECT slug FROM books WHERE id = ?1", [book_id], |r| r.get(0))
        .optional()
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let Some(slug) = slug else { return Ok(HashMap::new()) };
    // book-scoped tags for THIS book, plus word-scoped (global, book_slug='*') tags
    // for any of the book's words.
    let mut stmt = conn
        .prepare(
            "SELECT w.id, t.tag FROM u.word_tags t JOIN words w ON w.word = t.word
             WHERE (t.book_slug = ?1 OR t.book_slug = '*') AND t.rater = 'me'",
        )
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let mut map: HashMap<i64, Vec<String>> = HashMap::new();
    for r in stmt
        .query_map([slug], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .filter_map(Result::ok)
    {
        map.entry(r.0).or_default().push(r.1);
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
            // 'interesting'-class tag — book-scoped to this book, or word-scoped
            // (global) AND actually present in this book.
            "SELECT b.id, COALESCE(b.title, b.slug),
                    (SELECT count(DISTINCT t.word) FROM u.word_tags t
                       JOIN u.tags g ON g.name = t.tag
                       LEFT JOIN words w ON w.word = t.word
                       LEFT JOIN book_occurrences bo ON bo.word_id = w.id AND bo.book_id = b.id
                     WHERE g.interest = 'interesting'
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
            c.tags = t.clone();
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

#[server]
pub async fn set_tag(book_id: i64, word_id: i64, tag: String, on: bool) -> Result<(), ServerFnError> {
    use rusqlite::OptionalExtension;
    if !tag_allowed(&tag) {
        return Err(ServerFnError::new("invalid tag"));
    }
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
    let is_pick = tag.starts_with("pick:");
    // auto-register a brand-new custom tag into the collection (so its scope exists)
    if on && !is_pick {
        conn.execute(
            "INSERT OR IGNORE INTO u.tags(name, sort, created) VALUES (?1, 100, datetime('now'))",
            rusqlite::params![tag],
        )
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    }
    // a word-scoped tag is keyed by the '*' sentinel (it applies across all books);
    // pick: tags and book-scoped tags key by the real slug.
    let scope: String = if is_pick {
        SCOPE_BOOK.to_string()
    } else {
        conn.query_row("SELECT scope FROM u.tags WHERE name = ?1", [&tag], |r| r.get(0))
            .optional()
            .map_err(|e| ServerFnError::new(e.to_string()))?
            .unwrap_or_else(|| SCOPE_BOOK.to_string())
    };
    let key_slug = if scope == SCOPE_WORD { "*".to_string() } else { slug };
    if on {
        conn.execute(
            "INSERT OR IGNORE INTO u.word_tags(book_slug, word, tag, rater, ts)
             VALUES (?1, ?2, ?3, 'me', datetime('now'))",
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

/// All tags for a book, keyed by word_id — used to seed the client store so tag
/// state is known for every surface form (not just the candidate representatives),
/// which makes the family union (cross-level visibility) reactive.
#[server]
pub async fn book_tags(book_id: i64) -> Result<Vec<(i64, Vec<String>)>, ServerFnError> {
    let conn = open_conn()?;
    Ok(load_tags(&conn, book_id)?.into_iter().collect())
}

/// The user's whole tag collection (builtin defaults + custom), for the picker.
#[server]
pub async fn list_tags() -> Result<Vec<TagDef>, ServerFnError> {
    let conn = open_user()?;
    let mut stmt = conn
        .prepare("SELECT name, comment, builtin, scope, interest, sort FROM tags ORDER BY sort, name")
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

/// Rename a tag, cascading to all its applications (text-keyed). `star` is locked.
#[server]
pub async fn rename_tag(old: String, new: String) -> Result<String, ServerFnError> {
    use rusqlite::OptionalExtension;
    if old == "star" {
        return Err(ServerFnError::new("the ★ tag can't be renamed"));
    }
    let clean = sanitize_tag(&new).ok_or_else(|| ServerFnError::new("invalid tag name"))?;
    if clean == old {
        return Ok(clean);
    }
    let mut conn = open_user()?;
    let exists = conn
        .query_row("SELECT 1 FROM tags WHERE name = ?1", [&clean], |_| Ok(()))
        .optional()
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .is_some();
    if exists {
        return Err(ServerFnError::new("a tag with that name already exists"));
    }
    let tx = conn.transaction().map_err(|e| ServerFnError::new(e.to_string()))?;
    tx.execute("UPDATE tags SET name = ?1 WHERE name = ?2", rusqlite::params![clean, old])
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    tx.execute("UPDATE word_tags SET tag = ?1 WHERE tag = ?2", rusqlite::params![clean, old])
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    tx.commit().map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(clean)
}

/// Delete a tag and all its applications; returns how many applications were removed.
#[server]
pub async fn delete_tag(name: String) -> Result<i64, ServerFnError> {
    if name == "star" {
        return Err(ServerFnError::new("the ★ tag can't be deleted"));
    }
    let mut conn = open_user()?;
    let tx = conn.transaction().map_err(|e| ServerFnError::new(e.to_string()))?;
    let n = tx
        .execute("DELETE FROM word_tags WHERE tag = ?1", [&name])
        .map_err(|e| ServerFnError::new(e.to_string()))? as i64;
    tx.execute("DELETE FROM tags WHERE name = ?1", [&name])
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
        tx.execute(
            "INSERT OR IGNORE INTO u.word_tags(book_slug, word, tag, rater, ts)
             SELECT '*', word, tag, rater, ts FROM u.word_tags WHERE tag = ?1 AND book_slug <> '*'",
            [&name],
        )
        .map_err(|e| ServerFnError::new(e.to_string()))?;
        tx.execute("DELETE FROM u.word_tags WHERE tag = ?1 AND book_slug <> '*'", [&name])
            .map_err(|e| ServerFnError::new(e.to_string()))?;
    } else if let Some(slug) = convert_slug {
        // word→book, "just added": re-home the global rows to the current book.
        tx.execute(
            "INSERT OR IGNORE INTO u.word_tags(book_slug, word, tag, rater, ts)
             SELECT ?2, word, tag, rater, ts FROM u.word_tags WHERE tag = ?1 AND book_slug = '*'",
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

/// Move a tag one step up/down within its scope group (dense-renumbers the group).
#[server]
pub async fn reorder_tag(name: String, up: bool) -> Result<(), ServerFnError> {
    use rusqlite::OptionalExtension;
    let mut conn = open_user()?;
    let Some(scope) = conn
        .query_row("SELECT scope FROM tags WHERE name = ?1", [&name], |r| r.get::<_, String>(0))
        .optional()
        .map_err(|e| ServerFnError::new(e.to_string()))?
    else {
        return Ok(());
    };
    let mut stmt = conn
        .prepare("SELECT name FROM tags WHERE scope = ?1 ORDER BY sort, name")
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let mut names: Vec<String> = stmt
        .query_map([&scope], |r| r.get::<_, String>(0))
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .filter_map(Result::ok)
        .collect();
    drop(stmt); // release the borrow on conn before opening the transaction
    let Some(i) = names.iter().position(|n| n == &name) else { return Ok(()) };
    let j = if up { i.checked_sub(1) } else { (i + 1 < names.len()).then_some(i + 1) };
    let Some(j) = j else { return Ok(()) }; // already at the edge
    names.swap(i, j);
    let tx = conn.transaction().map_err(|e| ServerFnError::new(e.to_string()))?;
    for (k, n) in names.iter().enumerate() {
        tx.execute("UPDATE tags SET sort = ?1 WHERE name = ?2", rusqlite::params![k as i64, n])
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
    text_source: String,
    engine: String,
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
    if !text_source.is_empty() {
        args.push("--text-source".into());
        args.push(text_source);
    }
    if !engine.is_empty() {
        args.push("--engine".into());
        args.push(engine);
    }
    let argref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let v = run_importer(&argref)?;
    let res: ImportResult =
        serde_json::from_value(v).map_err(|e| ServerFnError::new(format!("parse result: {e}")))?;
    let _ = std::fs::remove_file(&staged);
    // drop any OCR cache sidecars (<token>.ocr.<engine>.json) left in staging
    if let Ok(rd) = std::fs::read_dir(staging_dir()) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with(&token) && name != token {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    Ok(res)
}

/// OCR a sample of a staged PDF's pages and diff against its embedded text layer,
/// so the user can judge whether to import the embedded text or our re-OCR.
#[server]
pub async fn ocr_compare(token: String, engine: String) -> Result<OcrCompareResult, ServerFnError> {
    if token.is_empty() || token.contains('/') || token.contains('\\') || token.contains("..") {
        return Err(ServerFnError::new("invalid upload token"));
    }
    let staged = staging_dir().join(&token);
    if !staged.exists() {
        return Err(ServerFnError::new("upload expired — please drop the file again"));
    }
    let path = staged.to_string_lossy().to_string();
    let mut args: Vec<String> = vec!["--ocr-compare".into(), path];
    if !engine.is_empty() {
        args.push("--engine".into());
        args.push(engine);
    }
    let argref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    // run_importer errors on {ok:false}; surface OCR-missing etc. as the error text.
    let v = run_importer(&argref)?;
    serde_json::from_value(v).map_err(|e| ServerFnError::new(format!("parse compare: {e}")))
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
fn has_tag(t: Tagger, key: (i64, i64), tag: &str) -> bool {
    t.store.with(|m| m.get(&key).is_some_and(|s| s.contains(tag)))
}
fn has_other_tags(t: Tagger, key: (i64, i64)) -> bool {
    t.store.with(|m| m.get(&key).is_some_and(|s| s.iter().any(|x| x != "star")))
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
        if next { set.insert(tag.to_string()); } else { set.remove(tag); }
    });
    t.action.dispatch(SetTag { book_id, word_id, tag: tag.to_string(), on: next });
}

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();
    view! {
        <Stylesheet id="leptos" href="/pkg/coolwords_ui.css"/>
        <Title text="coolwords — interesting words"/>
        <Router>
            <main>
                <Routes fallback=|| "Page not found.".into_view()>
                    <Route path=StaticSegment("") view=HomePage/>
                    <Route path=StaticSegment("tags") view=TagsPage/>
                    <Route path=StaticSegment("import") view=ImportPage/>
                    <Route path=StaticSegment("source") view=BookSourcePage/>
                </Routes>
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

/// The tag picker: the user's tag collection split into two drag groups — "this
/// book" (book-scoped, above) and "all books" (word-scoped, below) — coloured by
/// interest, plus per-word "good for: <bucket>" picks and an inline new-tag adder.
/// Dragging a chip between the groups changes that tag's scope.
#[component]
fn TagPicker(book_id: i64, word_id: i64, buckets: Vec<String>) -> impl IntoView {
    let t = expect_context::<Tagger>();
    let key = (book_id, word_id);
    let has_buckets = !buckets.is_empty();
    let new_name = RwSignal::new(String::new());
    let new_comment = RwSignal::new(String::new());
    let drag = RwSignal::new(None::<String>);
    let add_new = move || {
        let raw = new_name.get();
        let Some(clean) = sanitize_tag(&raw) else { return };
        t.add.dispatch(AddTag { name: raw, comment: new_comment.get() });
        if !has_tag(t, key, &clean) {
            toggle_tag(t, book_id, word_id, &clean);
        }
        new_name.set(String::new());
        new_comment.set(String::new());
    };

    // Change a dragged tag's scope per the local↔global rules.
    let move_scope = move |name: String, to_word: bool| {
        let cur_word = t.tags
            .with(|v| v.iter().find(|d| d.name == name).map(|d| d.scope == SCOPE_WORD))
            .unwrap_or(false);
        if cur_word == to_word {
            return;
        }
        if to_word {
            // local→global: non-destructive collapse; remember it's freshly global.
            t.just_global.update(|s| { s.insert(name.clone()); });
            t.tags.update(|v| { if let Some(d) = v.iter_mut().find(|d| d.name == name) { d.scope = SCOPE_WORD.into(); } });
            t.scope.dispatch(SetTagScope { name, scope: SCOPE_WORD.into(), convert_book: 0 });
        } else {
            // global→local: warn before dropping established global applications,
            // unless this tag was just added to global (then re-home to this book).
            let just = t.just_global.with(|s| s.contains(&name));
            leptos::task::spawn_local(async move {
                let n = tag_global_usage(name.clone()).await.unwrap_or(0);
                let (proceed, convert) = if n == 0 {
                    (true, 0i64)
                } else if just {
                    (true, book_id)
                } else {
                    let msg = format!(
                        "'{name}' is applied to {n} word(s) across all books.\nMake it book-only? This drops those applications.");
                    let ok = web_sys::window().and_then(|w| w.confirm_with_message(&msg).ok()).unwrap_or(false);
                    (ok, 0i64)
                };
                if proceed {
                    t.just_global.update(|s| { s.remove(&name); });
                    t.tags.update(|v| { if let Some(d) = v.iter_mut().find(|d| d.name == name) { d.scope = SCOPE_BOOK.into(); } });
                    t.scope.dispatch(SetTagScope { name, scope: SCOPE_BOOK.into(), convert_book: convert });
                }
            });
        }
    };

    // Reactive chips for one scope group (book or word), minus the ★ quick-tag.
    let chips = move |is_word: bool| {
        t.tags.get().into_iter()
            .filter(move |d| d.name != "star" && (d.scope == SCOPE_WORD) == is_word)
            .map(|d| {
                let name = d.name.clone();
                let on_name = name.clone();
                let click_name = name.clone();
                let drag_name = name.clone();
                let title = d.comment.clone().unwrap_or_default();
                let cls = format!("chip int-{}", d.interest);
                view! {
                    <button type="button" class=cls draggable="true" title=title
                        class:on=move || has_tag(t, key, &on_name)
                        on:dragstart=move |ev: web_sys::DragEvent| {
                            drag.set(Some(drag_name.clone()));
                            // a non-empty payload makes the drag valid in all browsers
                            if let Some(dt) = ev.data_transfer() { let _ = dt.set_data("text/plain", &drag_name); }
                        }
                        on:click=move |_| toggle_tag(t, book_id, word_id, &click_name)>
                        {name}
                    </button>
                }
            })
            .collect_view()
    };
    let drop_to = move |to_word: bool| {
        move |ev: web_sys::DragEvent| {
            ev.prevent_default();
            if let Some(n) = drag.get() {
                drag.set(None);
                move_scope(n, to_word);
            }
        }
    };

    view! {
        <div class="picker tagpick">
            <div class="scopegrp" on:dragover=move |ev| ev.prevent_default() on:drop=drop_to(false)>
                <span class="picklbl">"this book"</span>
                {move || chips(false)}
            </div>
            <div class="scopegrp global" on:dragover=move |ev| ev.prevent_default() on:drop=drop_to(true)>
                <span class="picklbl" title="word-scoped: applies to this word in every book">
                    "all books "<span class="draghint">"⇕ drag"</span>
                </span>
                {move || chips(true)}
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
                <input class="newtag-name" placeholder="+ new tag"
                    prop:value=move || new_name.get()
                    on:input=move |ev| new_name.set(event_target_value(&ev))
                    on:keydown=move |ev| if ev.key() == "Enter" { add_new(); }/>
                <input class="newtag-comment" placeholder="what it's for (optional)"
                    prop:value=move || new_comment.get()
                    on:input=move |ev| new_comment.set(event_target_value(&ev))
                    on:keydown=move |ev| if ev.key() == "Enter" { add_new(); }/>
                <button type="button" class="chip add" on:click=move |_| add_new()>"add"</button>
                <a class="managelink" href="/tags">"manage ↗"</a>
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
    let book = Memo::new(move |_| book_q.get().unwrap_or(1));
    let level = Memo::new(move |_| lvl_q.get().unwrap_or(0));
    let only_top = RwSignal::new(false);
    let hide_rejected = RwSignal::new(false);
    let open_picker = RwSignal::new(None::<i64>);

    let tagger = Tagger {
        store: RwSignal::new(HashMap::new()),
        action: ServerAction::<SetTag>::new(),
        tags: RwSignal::new(Vec::new()),
        add: ServerAction::<AddTag>::new(),
        scope: ServerAction::<SetTagScope>::new(),
        interest: ServerAction::<SetTagInterest>::new(),
        rename: ServerAction::<RenameTag>::new(),
        del: ServerAction::<DeleteTag>::new(),
        reorder: ServerAction::<ReorderTag>::new(),
        just_global: RwSignal::new(HashSet::new()),
    };
    provide_context(tagger);

    // The tag collection (builtin + custom), refetched after any tag-collection edit.
    let tag_defs = Resource::new(move || tagger_rev(tagger), |_| list_tags());
    Effect::new(move |_| {
        if let Some(Ok(defs)) = tag_defs.get() {
            tagger.tags.set(defs);
        }
    });

    let books = Resource::new(|| (), |_| list_books());
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
            <A href="/import" attr:class="importlink">"+ import book"</A>
            <a class="srclink" href=move || format!("/source?book={}", book.get())
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
            <a class="toggle managelink" href="/tags">"manage tags ↗"</a>
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
                        <table>
                            <thead><tr>
                                <th></th><th>"word"</th><th>"gloss"</th><th>"in bk"</th><th>"score"</th>
                                <th>"origin"</th><th>"category"</th><th>"cl"</th><th>"tags"</th>
                            </tr></thead>
                            <tbody>
                                {list.into_iter().map(|c| {
                                    let wid = c.word_id;
                                    let bk = c.buckets.clone();
                                    let star = if c.selected { "•" } else { "" };
                                    let nforms = c.n_forms;
                                    let members = c.members.clone();
                                    let members_has = c.members.clone();
                                    let members_rej = c.members.clone();
                                    let members_hid = c.members.clone();
                                    let gloss = short(&c.gloss, 90);
                                    let example = c.example.clone().unwrap_or_default();
                                    let origin_disp = c.origin_name.clone().or_else(|| c.origin_code.clone()).unwrap_or_default();
                                    let origin_title = c.origin_code.clone().unwrap_or_default();
                                    let cat = c.category.clone();
                                    let cat_click = cat.clone();
                                    let cluster_txt = c.cluster.map(|n| n.to_string()).unwrap_or_default();
                                    view! {
                                        <tr class="row"
                                            class:interesting=move || group_interest(tagger, b, &members) == Some(Interest::Interesting)
                                            class:rejected=move || group_interest(tagger, b, &members_rej) == Some(Interest::Uninteresting)
                                            class:hidden=move || hide_rejected.get() && group_interest(tagger, b, &members_hid) == Some(Interest::Uninteresting)>
                                            <td class="sel">{star}</td>
                                            <td class="word" title=example on:click=move |_| set_word.set(Some(wid))>
                                                {c.word.clone()}
                                                {(nforms > 1).then(|| view! {
                                                    <small class="forms" title="surface forms merged into this group at the current level">{format!(" +{}", nforms - 1)}</small>
                                                })}
                                            </td>
                                            <td class="gloss">{gloss}</td>
                                            <td class="num">{c.in_book}</td>
                                            <td class="num">{format!("{:.1}", c.score)}</td>
                                            <td title=origin_title>{origin_disp}</td>
                                            <td class="cat"
                                                on:click=move |_| { if let Some(cc) = cat_click.clone() { set_cat.set(Some(cc)); } }>
                                                {cat.clone().unwrap_or_default()}
                                            </td>
                                            <td class="num">{cluster_txt}</td>
                                            <td class="tagcell">
                                                <Star book_id=b word_id=wid/>
                                                <button type="button" class="tagbtn"
                                                    class:has=move || group_has_other(tagger, b, &members_has)
                                                    on:click=move |_| open_picker.update(|o| *o = if *o == Some(wid) { None } else { Some(wid) })>
                                                    "tags"
                                                </button>
                                                <Show when=move || open_picker.get() == Some(wid) fallback=|| ()>
                                                    <TagPicker book_id=b word_id=wid buckets=bk.clone()/>
                                                </Show>
                                            </td>
                                        </tr>
                                    }
                                }).collect_view()}
                            </tbody>
                        </table>
                    }.into_any()
                }
            })}
        </Suspense>

        <Show when=move || selected.get().is_some() fallback=|| ()>
            <aside class="detail">
                <button class="close" on:click=move |_| set_word.set(None)>"×"</button>
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

/// One editable row in the tag manager: reorder, rename, comment, scope, interest, delete.
#[component]
fn TagRow(
    d: TagDef,
    rename: ServerAction<RenameTag>,
    del: ServerAction<DeleteTag>,
    interest: ServerAction<SetTagInterest>,
    scope: ServerAction<SetTagScope>,
    reorder: ServerAction<ReorderTag>,
    comment_act: ServerAction<AddTag>,
) -> impl IntoView {
    let name = d.name.clone();
    let locked = name == "star";
    let is_word = d.scope == SCOPE_WORD;
    let cur_interest = d.interest.clone();
    let int_cls = d.interest.clone();
    let comment = d.comment.clone().unwrap_or_default();
    let (n_up, n_down, n_ren, n_com, n_int, n_scope, n_del) = (
        name.clone(), name.clone(), name.clone(), name.clone(), name.clone(), name.clone(), name.clone(),
    );

    let toggle_scope = move |_| {
        if !is_word {
            scope.dispatch(SetTagScope { name: n_scope.clone(), scope: SCOPE_WORD.into(), convert_book: 0 });
        } else {
            let nm = n_scope.clone();
            leptos::task::spawn_local(async move {
                let cnt = tag_global_usage(nm.clone()).await.unwrap_or(0);
                let ok = cnt == 0
                    || web_sys::window()
                        .and_then(|w| w.confirm_with_message(&format!(
                            "'{nm}' is applied to {cnt} word(s) across all books. Make it book-only? This drops those applications.")).ok())
                        .unwrap_or(false);
                if ok {
                    scope.dispatch(SetTagScope { name: nm, scope: SCOPE_BOOK.into(), convert_book: 0 });
                }
            });
        }
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
        <tr class=format!("tagrow int-{int_cls}")>
            <td class="reorder">
                <button class="mini" title="move up" on:click=move |_| { reorder.dispatch(ReorderTag { name: n_up.clone(), up: true }); }>"↑"</button>
                <button class="mini" title="move down" on:click=move |_| { reorder.dispatch(ReorderTag { name: n_down.clone(), up: false }); }>"↓"</button>
            </td>
            <td>
                {if locked {
                    view! { <span class="tagname locked" title="the quick ★ tag can't be renamed or deleted">{name.clone()}" ★"</span> }.into_any()
                } else {
                    view! { <input class="tagname" prop:value=name.clone()
                        on:change=move |ev| { let v = event_target_value(&ev);
                            if !v.is_empty() && v != n_ren { rename.dispatch(RenameTag { old: n_ren.clone(), new: v }); } }/> }.into_any()
                }}
            </td>
            <td>
                <input class="tagcomment" prop:value=comment placeholder="what it's for"
                    on:change=move |ev| { comment_act.dispatch(AddTag { name: n_com.clone(), comment: event_target_value(&ev) }); }/>
            </td>
            <td>
                <button class="scopebtn" class:global=is_word title="toggle book / word scope" on:click=toggle_scope>
                    {if is_word { "word — all books" } else { "book" }}
                </button>
            </td>
            <td>
                <select class="intsel" prop:value=cur_interest disabled=locked
                    on:change=move |ev| { interest.dispatch(SetTagInterest { name: n_int.clone(), interest: event_target_value(&ev) }); }>
                    <option value="interesting">"interesting · favourites"</option>
                    <option value="neutral">"neutral · note"</option>
                    <option value="uninteresting">"uninteresting · junk"</option>
                </select>
            </td>
            <td>
                {(!locked).then(|| view! { <button class="catx" title="delete tag" on:click=do_delete>"✕"</button> })}
            </td>
        </tr>
    }
}

/// The tag manager page: rename / delete / reorder / re-scope / set interest, grouped
/// into book-scoped and word-scoped tags.
#[component]
fn TagsPage() -> impl IntoView {
    let add = ServerAction::<AddTag>::new();
    let rename = ServerAction::<RenameTag>::new();
    let del = ServerAction::<DeleteTag>::new();
    let interest = ServerAction::<SetTagInterest>::new();
    let scope = ServerAction::<SetTagScope>::new();
    let reorder = ServerAction::<ReorderTag>::new();
    let rev = move || {
        add.version().get() + rename.version().get() + del.version().get()
            + interest.version().get() + scope.version().get() + reorder.version().get()
    };
    let tags = Resource::new(rev, |_| list_tags());
    let new_name = RwSignal::new(String::new());
    let new_comment = RwSignal::new(String::new());
    let add_new = move || {
        let raw = new_name.get();
        if sanitize_tag(&raw).is_some() {
            add.dispatch(AddTag { name: raw, comment: new_comment.get() });
            new_name.set(String::new());
            new_comment.set(String::new());
        }
    };

    let group = move |d: TagDef| view! {
        <TagRow d=d rename=rename del=del interest=interest scope=scope reorder=reorder comment_act=add/>
    };
    view! {
        <h1>"tags"</h1>
        <p class="sub"><a href="/">"← back to words"</a>
            " · interesting tags favourite a word; uninteresting tags mark it as junk (hideable)."</p>
        <Suspense fallback=move || view! { <p class="loading">"Loading…"</p> }>
            {move || tags.get().map(|res| match res {
                Err(e) => view! { <p class="err">{format!("{e}")}</p> }.into_any(),
                Ok(list) => {
                    let (bk, wd): (Vec<_>, Vec<_>) = list.into_iter().partition(|d| d.scope != SCOPE_WORD);
                    view! {
                        <table class="tagtable">
                            <thead><tr>
                                <th></th><th>"tag"</th><th>"comment / what it's for"</th>
                                <th>"scope"</th><th>"interest"</th><th></th>
                            </tr></thead>
                            <tbody>
                                <tr class="grouphdr"><td colspan="6">"book tags — a word in one book"</td></tr>
                                {bk.into_iter().map(group).collect_view()}
                                <tr class="grouphdr"><td colspan="6">"word tags — a word across all books"</td></tr>
                                {wd.into_iter().map(group).collect_view()}
                            </tbody>
                        </table>
                    }.into_any()
                }
            })}
        </Suspense>
        <div class="pickgroup newtag tagsadd">
            <input class="newtag-name" placeholder="+ new tag" prop:value=move || new_name.get()
                on:input=move |ev| new_name.set(event_target_value(&ev))
                on:keydown=move |ev| if ev.key() == "Enter" { add_new(); }/>
            <input class="newtag-comment" placeholder="what it's for (optional)" prop:value=move || new_comment.get()
                on:input=move |ev| new_comment.set(event_target_value(&ev))
                on:keydown=move |ev| if ev.key() == "Enter" { add_new(); }/>
            <button class="chip add" on:click=move |_| add_new()>"add tag"</button>
        </div>
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
    let f_source = RwSignal::new(String::from("embedded")); // PDF text source
    let f_engine = RwSignal::new(String::new());            // OCR engine
    let compare = RwSignal::new(CompareState::Idle);
    let committing = RwSignal::new(false);
    let commit_err = RwSignal::new(None::<String>);
    let file_input: NodeRef<leptos::html::Input> = NodeRef::new();

    // Seed the editable form (and PDF source/engine) from each fresh inspection.
    Effect::new(move |_| {
        if let UploadState::Done(insp) = state.get() {
            f_title.set(insp.title.clone());
            f_author.set(insp.author.clone());
            f_year.set(insp.year.map(|y| y.to_string()).unwrap_or_default());
            f_slug.set(insp.suggested_slug.clone());
            f_source.set(if insp.needs_ocr { "ocr".into() } else { "embedded".into() });
            f_engine.set(insp.ocr.as_ref().map(|o| o.default_engine.clone()).unwrap_or_default());
            compare.set(CompareState::Idle);
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

    let do_compare = move |_| {
        let UploadState::Done(insp) = state.get() else { return };
        let token = insp.token.clone();
        let engine = f_engine.get();
        compare.set(CompareState::Loading);
        leptos::task::spawn_local(async move {
            match ocr_compare(token, engine).await {
                Ok(c) => compare.set(CompareState::Done(Box::new(c))),
                Err(e) => compare.set(CompareState::Failed(e.to_string())),
            }
        });
    };

    let do_commit = move |_| {
        let UploadState::Done(insp) = state.get() else { return };
        committing.set(true);
        commit_err.set(None);
        let token = insp.token.clone();
        let orig = insp.orig_filename.clone();
        let (title, author, year, slug) = (f_title.get(), f_author.get(), f_year.get(), f_slug.get());
        // text_source/engine only matter for PDFs; harmlessly ignored otherwise.
        let (source, engine) = if insp.pdf.is_some() {
            (f_source.get(), f_engine.get())
        } else {
            (String::new(), String::new())
        };
        let navigate = use_navigate();
        leptos::task::spawn_local(async move {
            match confirm_import(token, slug, title, author, year, orig, source, engine).await {
                Ok(res) => navigate(&format!("/?book={}", res.book_id), Default::default()),
                Err(e) => {
                    commit_err.set(Some(e.to_string()));
                    committing.set(false);
                }
            }
        });
    };

    view! {
        <h1>"Import a book"</h1>
        <p class="sub"><A href="/">"← back to words"</A>
            " · drop a .txt, .epub or .pdf; we detect title/author and show what gets stripped. "
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
                let engines_avail: Vec<String> = insp.ocr.as_ref().map(|o| {
                    let mut v: Vec<String> = o.engines.iter().filter(|(_, e)| e.available)
                        .map(|(k, _)| k.clone()).collect();
                    v.sort();
                    v
                }).unwrap_or_default();
                let engines_missing: Vec<String> = insp.ocr.as_ref().map(|o| {
                    o.engines.iter().filter(|(_, e)| !e.available)
                        .map(|(k, e)| format!("{k}: {}", e.detail)).collect()
                }).unwrap_or_default();
                let has_engines = !engines_avail.is_empty();
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
                                    "No embedded text layer — this looks like a scan, so text will come from OCR."
                                </div>
                            })}
                            <div class="ocr-controls">
                                <span class="picklbl">"text source: "</span>
                                <label><input type="radio" name="src" disabled=needs_ocr
                                    prop:checked=move || f_source.get() == "embedded"
                                    on:change=move |_| f_source.set("embedded".into())/>" embedded"</label>
                                <label><input type="radio" name="src"
                                    prop:checked=move || f_source.get() == "ocr"
                                    on:change=move |_| f_source.set("ocr".into())/>" re-OCR"</label>
                                {has_engines.then(|| view! {
                                    <select class="intsel" prop:value=move || f_engine.get()
                                        on:change=move |e| f_engine.set(event_target_value(&e))>
                                        {engines_avail.clone().into_iter()
                                            .map(|n| { let v = n.clone(); view! { <option value=v>{n}</option> } }).collect_view()}
                                    </select>
                                    <button type="button" class="chip" on:click=do_compare>
                                        "Compare OCR (sample)"</button>
                                })}
                            </div>
                            {(!engines_missing.is_empty()).then(|| view! {
                                <p class="yr-note">{format!("OCR engines not installed — {}", engines_missing.join("; "))}</p>
                            })}
                            {move || match compare.get() {
                                CompareState::Idle => ().into_any(),
                                CompareState::Loading => view! {
                                    <p class="loading">"OCR-ing sample pages…"</p> }.into_any(),
                                CompareState::Failed(e) => view! { <p class="err">{e}</p> }.into_any(),
                                CompareState::Done(c) => view! { <OcrCompareView cmp=*c/> }.into_any(),
                            }}
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
        <p class="sub"><A href="/">"← back to words"</A></p>
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
