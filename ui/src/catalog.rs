//! Catalog browsing: the synced Standard Ebooks / Project Gutenberg catalogs live
//! in SQLite (the sync is an explicit user action, never a per-search fetch), and
//! this module is the UI's read side over them — search/filter/page the rows, show
//! which ones we already imported, and queue downloads onto the background job
//! registry for the bulk "grab" flow.
//!
//! Search is answered from Rust, not by shelling out to `ingest.catalog --search`:
//! it's one table and the page searches as you type, so a python startup per
//! keystroke would dominate. The SQL below deliberately mirrors `do_search` in
//! ingest/catalog.py (same WHERE, same sorts, same relevance CASE) so the CLI and
//! the UI can never disagree about what "matches".
//!
//! Everything that reaches a subprocess argument (a source name, a source id) is
//! validated against an allowlist here, even though this is a single-user tool
//! behind Home Assistant ingress.
//!
//! The shared serde types are deliberately NOT behind `cfg(ssr)` — the hydrated
//! client deserializes them out of server-fn responses.

use std::collections::HashSet;

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

use crate::app::JobProgress;

/// The two catalogs we know how to sync. Also the allowlist every `source` string
/// is checked against before it becomes a subprocess argument.
pub const SOURCES: [&str; 2] = ["standardebooks", "gutenberg"];

/// Human label for a source id. Unknown ids echo back verbatim (they can only come
/// from the DB, never from a request, since requests are allowlisted).
pub fn source_label(source: &str) -> &str {
    match source {
        "standardebooks" => "Standard Ebooks",
        "gutenberg" => "Project Gutenberg",
        other => other,
    }
}

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
    /// The imported book's row id, so the "already imported" label can link straight
    /// at `/?book=<id>` (the app addresses books by id, not slug).
    pub imported_book_id: Option<i64>,
}

/// One page of catalog results plus the unpaged total, so the UI can show
/// "showing 48 of 1,492" without a second round trip.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct CatalogPage {
    pub total: i64,
    #[serde(default)]
    pub items: Vec<CatalogEntry>,
}

/// Freshness for ONE source: how many rows we hold, how many of them we've already
/// imported, and how long ago the sync ran (pre-formatted — the client only prints
/// it, and "2 days ago" needs a clock the client shouldn't have to agree with).
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct CatalogSourceStatus {
    pub source: String,
    pub label: String,
    pub rows: i64,
    pub imported: i64,
    /// "2 days ago", or "" when this source has never been synced.
    pub synced: String,
}

/// Freshness summary for the sync row. A Vec rather than per-source fields so the
/// component loops over [`SOURCES`] instead of hard-coding two of everything.
///
/// (Named …Summary, not …Status: the `#[server] catalog_status` fn below makes the
/// macro derive an args struct called `CatalogStatus`, which would collide. Same
/// dance as `OcrStatus` vs `BookOcrStatus` in app.rs.)
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct CatalogSummary {
    #[serde(default)]
    pub sources: Vec<CatalogSourceStatus>,
}

impl CatalogSummary {
    /// True when nothing has ever been synced — the page then leads with "sync
    /// first" instead of an empty result list that looks like a broken search.
    pub fn is_empty(&self) -> bool {
        self.sources.iter().all(|s| s.rows == 0)
    }
}

/// 61369 -> "61,369". Row counts and word counts are unreadable without separators.
pub fn commas(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if n < 0 {
        out.push('-');
    }
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

// ------------------------------------------------------------------ server ---- #

/// schema/catalog.sql is all `IF NOT EXISTS`, so applying it on open is idempotent
/// and means /get works on a database that predates the catalog feature (ingest/db.py
/// would only create the tables on the next python run, which may never happen if
/// the user's whole workflow is now "browse and grab").
#[cfg(feature = "ssr")]
pub(crate) const CATALOG_SCHEMA: &str = include_str!("../../schema/catalog.sql");

#[cfg(feature = "ssr")]
fn open_catalog() -> Result<rusqlite::Connection, ServerFnError> {
    let conn = crate::app::open_conn()?;
    conn.execute_batch(CATALOG_SCHEMA)
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(conn)
}

/// Reject anything that isn't one of the two catalogs. `all` is accepted only where
/// the python side accepts it (sync), never where it becomes an identity.
#[cfg(feature = "ssr")]
fn check_source(source: &str) -> Result<(), ServerFnError> {
    if SOURCES.contains(&source) {
        Ok(())
    } else {
        Err(ServerFnError::new(format!("unknown catalog source {source:?}")))
    }
}

/// A source id is a Gutenberg `Text#` or a Standard Ebooks `<author>/<title>` path
/// fragment. It ends up in a JSON payload the importer turns into a URL and a
/// filename, so: lowercase-ish path characters only, no traversal, bounded length.
#[cfg(feature = "ssr")]
fn valid_source_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 200
        && !id.contains("..")
        && !id.starts_with('/')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'))
}

/// Seconds-since-sync -> "3 hours ago". Coarse on purpose: the only decision it
/// informs is "is this stale enough to re-sync?".
#[cfg(feature = "ssr")]
fn ago(secs: i64) -> String {
    let s = secs.max(0);
    let (n, unit) = match s {
        0..=90 => return "just now".to_string(),
        s if s < 5400 => (s / 60, "minute"),
        s if s < 172_800 => (s / 3600, "hour"),
        s if s < 5_184_000 => (s / 86400, "day"),
        s => (s / 2_592_000, "month"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

/// Per-source row counts, how many are already imported, and how long ago each
/// catalog was synced.
#[server]
pub async fn catalog_status() -> Result<CatalogSummary, ServerFnError> {
    let conn = open_catalog()?;
    let mut sources = Vec::with_capacity(SOURCES.len());
    for src in SOURCES {
        let rows: i64 = conn
            .query_row("SELECT count(*) FROM catalog_books WHERE source = ?1", [src], |r| r.get(0))
            .map_err(|e| ServerFnError::new(e.to_string()))?;
        // The same (source, source_id) join the search uses, counted rather than listed.
        let imported: i64 = conn
            .query_row(
                "SELECT count(*) FROM catalog_books c JOIN books b \
                   ON b.source = c.source AND b.source_id = c.source_id \
                 WHERE c.source = ?1",
                [src],
                |r| r.get(0),
            )
            .map_err(|e| ServerFnError::new(e.to_string()))?;
        // synced_at is written by python as UTC `datetime('now')`; let SQLite do the
        // arithmetic so we never have to agree on a timezone with it.
        let age: Option<i64> = conn
            .query_row(
                "SELECT CAST((julianday('now') - julianday(synced_at)) * 86400 AS INTEGER) \
                 FROM catalog_sync WHERE source = ?1",
                [src],
                |r| r.get(0),
            )
            .unwrap_or(None);
        sources.push(CatalogSourceStatus {
            source: src.to_string(),
            label: source_label(src).to_string(),
            rows,
            imported,
            synced: age.map(ago).unwrap_or_default(),
        });
    }
    Ok(CatalogSummary { sources })
}

/// The distinct subjects present, most-used first, for the filter dropdown.
///
/// `subjects` is a "; "-joined text column (that's how both upstreams give it), so
/// there's no way to index this — we scan and split. It's ~60k short strings and the
/// result is fetched once per source change, not per search.
#[server]
pub async fn catalog_subjects(source: String) -> Result<Vec<(String, i64)>, ServerFnError> {
    use std::collections::HashMap;
    let conn = open_catalog()?;
    let mut sql =
        "SELECT subjects FROM catalog_books WHERE subjects IS NOT NULL AND subjects <> ''"
            .to_string();
    let mut params: Vec<rusqlite::types::Value> = Vec::new();
    if !source.is_empty() {
        check_source(&source)?;
        sql.push_str(" AND source = ?");
        params.push(rusqlite::types::Value::Text(source));
    }
    let mut stmt = conn.prepare(&sql).map_err(|e| ServerFnError::new(e.to_string()))?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(params.iter()), |r| r.get::<_, String>(0))
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let mut counts: HashMap<String, i64> = HashMap::new();
    for row in rows.flatten() {
        for s in row.split(';') {
            let s = s.trim();
            // PG headings run long ("Whaling -- Fiction"); anything longer than this
            // is a data-quality artefact and would blow out the dropdown.
            if !s.is_empty() && s.len() <= 60 {
                *counts.entry(s.to_string()).or_insert(0) += 1;
            }
        }
    }
    let mut out: Vec<(String, i64)> = counts.into_iter().collect();
    // Top by frequency (the long tail is one-offs), then alphabetical so the list
    // reads like a list instead of a leaderboard.
    out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    out.truncate(120);
    out.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    Ok(out)
}

/// Search/browse the local catalog. Mirrors `ingest/catalog.py:do_search`.
#[server]
pub async fn search_catalog(
    query: String,
    source: String,
    subject: String,
    sort: String,
    limit: i64,
    offset: i64,
) -> Result<CatalogPage, ServerFnError> {
    use rusqlite::types::Value as V;

    let query = query.trim().to_lowercase();
    if !source.is_empty() {
        check_source(&source)?;
    }
    // Cap server-side: `limit` arrives from the client and drives an allocation.
    let limit = limit.clamp(1, 200);
    let offset = offset.max(0);

    // `sort` is interpolated into the SQL (it's a column list, not a value), so it
    // must be looked up, never passed through.
    let sort_sql = match sort.as_str() {
        "title" => Some("c.title COLLATE NOCASE ASC"),
        "author" => Some("c.author COLLATE NOCASE ASC, c.title COLLATE NOCASE ASC"),
        "year" => Some("c.year IS NULL, c.year ASC, c.title COLLATE NOCASE ASC"),
        "words" => Some("c.n_words IS NULL, c.n_words DESC, c.title COLLATE NOCASE ASC"),
        _ => None, // "relevance" (or anything unrecognised) falls through below
    };

    let mut clauses: Vec<&str> = Vec::new();
    let mut params: Vec<V> = Vec::new();
    if !source.is_empty() {
        clauses.push("c.source = ?");
        params.push(V::Text(source.clone()));
    }
    if !query.is_empty() {
        clauses.push("(lower(c.title) LIKE ? OR lower(c.author) LIKE ?)");
        params.push(V::Text(format!("%{query}%")));
        params.push(V::Text(format!("%{query}%")));
    }
    if !subject.is_empty() {
        // Whole-entry match, not a bare substring: `catalog_subjects` counts whole
        // "; "-delimited entries, so the number beside the option has to be the number
        // this filter returns. LIKE '%fiction%' would additionally swallow every
        // "Whaling -- Fiction" heading and report ~30,000 results for an option
        // labelled "Fiction (4,102)". Normalising "; " to ";" and wrapping the column
        // in sentinel semicolons is what anchors the first and last entries.
        clauses.push("';' || replace(lower(c.subjects), '; ', ';') || ';' LIKE ?");
        params.push(V::Text(format!("%;{};%", subject.to_lowercase())));
    }
    let where_sql = if clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses.join(" AND "))
    };

    let conn = open_catalog()?;
    let total: i64 = conn
        .query_row(
            &format!("SELECT count(*) FROM catalog_books c{where_sql}"),
            rusqlite::params_from_iter(params.iter()),
            |r| r.get(0),
        )
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    // ORDER BY sits after WHERE, so its placeholders bind after the WHERE ones.
    let mut order_params: Vec<V> = Vec::new();
    let order: String = match (sort_sql, query.is_empty()) {
        (Some(s), _) => s.to_string(),
        (None, true) => "c.title COLLATE NOCASE ASC".to_string(),
        (None, false) => {
            // Relevance-ish: exact title, then title prefix, then title substring,
            // then author-only hits — enough to float "Moby Dick" above "…Moby Dick…".
            order_params.push(V::Text(query.clone()));
            order_params.push(V::Text(format!("{query}%")));
            order_params.push(V::Text(format!("%{query}%")));
            "CASE WHEN lower(c.title) = ? THEN 0 \
                  WHEN lower(c.title) LIKE ? THEN 1 \
                  WHEN lower(c.title) LIKE ? THEN 2 \
                  ELSE 3 END, c.title COLLATE NOCASE ASC"
                .to_string()
        }
    };

    let sql = format!(
        // Keeping the catalog in coolwords.db is what lets one query say both "here
        // are the matches" and "these are already in", so the UI can grey them out
        // instead of offering a no-op download. Correlated subqueries, NOT a LEFT
        // JOIN: `books` has no UNIQUE(source, source_id) — the same PG book dropped
        // in as .txt and as .epub lands twice with different content_hash — and a
        // join would fan one catalog row out into several, rendering the title twice
        // and desyncing this page window from `total`, which is counted without the
        // join (so the tail of the result set becomes unreachable in the pager).
        // min-by-id picks the first import, which is the one the user would expect.
        "SELECT c.source, c.source_id, c.title, c.author, c.year, c.language, c.subjects, \
                c.n_words, c.reading_ease, c.fmt, c.url, \
                (SELECT b.slug FROM books b \
                  WHERE b.source = c.source AND b.source_id = c.source_id \
                  ORDER BY b.id LIMIT 1), \
                (SELECT b.id FROM books b \
                  WHERE b.source = c.source AND b.source_id = c.source_id \
                  ORDER BY b.id LIMIT 1) \
         FROM catalog_books c\
         {where_sql} ORDER BY {order} LIMIT ? OFFSET ?"
    );
    let mut all: Vec<V> = params;
    all.extend(order_params);
    all.push(V::Integer(limit));
    all.push(V::Integer(offset));

    let mut stmt = conn.prepare(&sql).map_err(|e| ServerFnError::new(e.to_string()))?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(all.iter()), |r| {
            let subjects: String = r.get::<_, Option<String>>(6)?.unwrap_or_default();
            Ok(CatalogEntry {
                source: r.get(0)?,
                source_id: r.get(1)?,
                title: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                author: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                year: r.get(4)?,
                language: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
                subjects: subjects
                    .split(';')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect(),
                n_words: r.get(7)?,
                reading_ease: r.get(8)?,
                fmt: r.get::<_, Option<String>>(9)?.unwrap_or_default(),
                url: r.get::<_, Option<String>>(10)?.unwrap_or_default(),
                imported_slug: r.get(11)?,
                imported_book_id: r.get(12)?,
            })
        })
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let items: Vec<CatalogEntry> = rows.filter_map(Result::ok).collect();
    Ok(CatalogPage { total, items })
}

/// Kick off a catalog refresh from upstream as a background job. `source` becomes a
/// subprocess argument, so it is allowlisted (plus "all", which the CLI accepts).
#[server]
pub async fn sync_catalog(source: String) -> Result<String, ServerFnError> {
    if source != "all" {
        check_source(&source)?;
    }
    let label = if source == "all" {
        "syncing all catalogs…".to_string()
    } else {
        format!("syncing {}…", source_label(&source))
    };
    // book_id 0 + tag=source: a second click while one is running returns the same
    // job instead of hammering a volunteer-run server twice.
    Ok(crate::jobs::start_module(
        "catalog",
        "ingest.catalog",
        0,
        &source,
        &label,
        vec!["--sync".to_string(), source.clone()],
    ))
}

/// Largest bulk grab we'll queue in one go. Well past any realistic selection, and
/// low enough that a mis-click can't start a 60,000-book download.
pub const MAX_GRAB: usize = 200;

/// Which of these (source, source_id) pairs are already in `books`, as
/// (source, source_id) -> (book_id, slug).
#[cfg(feature = "ssr")]
fn imported_map(
    items: &[(String, String)],
) -> Result<std::collections::HashMap<(String, String), (i64, String)>, ServerFnError> {
    use rusqlite::OptionalExtension;
    let conn = crate::app::open_conn()?;
    let mut stmt = conn
        .prepare("SELECT id, slug FROM books WHERE source = ?1 AND source_id = ?2")
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let mut out = std::collections::HashMap::new();
    for (src, sid) in items {
        let row: Option<(i64, String)> = stmt
            .query_row([src.as_str(), sid.as_str()], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()
            .map_err(|e| ServerFnError::new(e.to_string()))?;
        if let Some(hit) = row {
            out.insert((src.clone(), sid.clone()), hit);
        }
    }
    Ok(out)
}

/// Queue a bulk download+import, then queue one scoring job per book that landed.
///
/// Two phases on purpose. The grab job downloads and ingests everything back to back
/// (python skips scoring: `do_grab` passes `run_pipeline=False`), so every title shows
/// up in the library within a couple of minutes. Scoring is the expensive part, so it
/// runs afterwards as one job PER BOOK through jobs.rs's single-slot gate — book 1 is
/// usable while book 20 is still waiting, instead of nothing being usable for an hour.
///
/// We work out "which books did the grab actually create?" by diffing `books` before
/// and after rather than by reading the grab's JSON, which keeps jobs.rs (whose only
/// output channel is the progress snapshot) untouched. The diff is also strictly more
/// correct than trusting the payload: a content-hash duplicate or an already-imported
/// row is in the grab's `skipped` bucket and rightly doesn't get re-scored.
#[server]
pub async fn grab_books(items: Vec<(String, String)>) -> Result<String, ServerFnError> {
    if items.is_empty() {
        return Err(ServerFnError::new("nothing selected"));
    }
    if items.len() > MAX_GRAB {
        return Err(ServerFnError::new(format!(
            "{} books selected — {MAX_GRAB} at a time, please",
            items.len()
        )));
    }
    let mut clean: Vec<(String, String)> = Vec::with_capacity(items.len());
    for (src, sid) in &items {
        check_source(src)?;
        if !valid_source_id(sid) {
            return Err(ServerFnError::new(format!("bad catalog id {sid:?}")));
        }
        clean.push((src.clone(), sid.clone()));
    }
    clean.sort();
    clean.dedup();

    // Snapshot first: anything already here must not be re-scored afterwards.
    let before = imported_map(&clean)?;

    let payload: Vec<serde_json::Value> = clean
        .iter()
        .map(|(s, i)| serde_json::json!({ "source": s, "source_id": i }))
        .collect();
    let json = serde_json::to_string(&payload).map_err(|e| ServerFnError::new(e.to_string()))?;
    // 200 Standard Ebooks ids is comfortably longer than a Windows command line likes,
    // so the list goes through a file — which is also what --grab-file exists for.
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let path = crate::app::staging_dir().join(format!("grab-{stamp}.json"));
    std::fs::write(&path, &json)
        .map_err(|e| ServerFnError::new(format!("could not stage the grab list: {e}")))?;

    let label = format!("downloading {} book{}…", clean.len(), if clean.len() == 1 { "" } else { "s" });
    // The tag is the batch itself (as a cheap digest), so a double-click on "download"
    // returns the running job while a genuinely different selection queues behind it.
    let tag = format!("{:x}", digest(&json));
    let id = crate::jobs::start_module(
        "grab",
        "ingest.catalog",
        0,
        &tag,
        &label,
        vec!["--grab-file".to_string(), path.to_string_lossy().into_owned()],
    );

    // Watch the grab from a blocking worker (rusqlite is blocking anyway, and the
    // crate doesn't enable tokio's "time" feature so there's no async sleep here).
    // The runtime handle is entered explicitly so jobs::start_module's inner
    // tokio::spawn is guaranteed a runtime.
    let handle = tokio::runtime::Handle::current();
    let watch_id = id.clone();
    tokio::task::spawn_blocking(move || {
        let _guard = handle.enter();
        let ended = loop {
            std::thread::sleep(std::time::Duration::from_millis(1000));
            match crate::jobs::status(&watch_id) {
                Some(p) if matches!(p.status.as_str(), "queued" | "running") => continue,
                Some(p) => break p.status, // done, failed, or cancelled
                None => break "reaped".to_string(),
            }
        };
        let _ = std::fs::remove_file(&path);
        // A cancelled grab is the user asking us to STOP — and `stop_queue` cancels
        // the grab a full poll tick before we get here, so fanning out now would
        // refill the queue it just emptied with an hour of scoring, one job per book
        // that had landed. Books that did land are still in the library and can be
        // scored on demand from the books page (`start_rescore` exists for this).
        if ended == "cancelled" {
            return;
        }
        // Even a FAILED grab usually imported SOMETHING; score whatever landed.
        let Ok(after) = imported_map(&clean) else { return };
        let mut fresh: Vec<(i64, String)> = after
            .into_iter()
            .filter(|(key, _)| !before.contains_key(key))
            .map(|(_, v)| v)
            .collect();
        fresh.sort(); // by book_id: score them in the order they were imported
        for (book_id, slug) in fresh {
            crate::jobs::start_module(
                "rescore",
                "ingest.import_book",
                book_id,
                "",
                &format!("scoring {slug}…"),
                vec!["--rescore".to_string(), slug],
            );
        }
    });

    Ok(id)
}

/// FNV-1a over the batch JSON — just a stable short name for "this exact selection",
/// used as the job's dedup tag. Not security-relevant.
#[cfg(feature = "ssr")]
fn digest(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h
}

/// Queue the scoring pipeline for one already-imported book. Exposed on its own so a
/// book whose scoring failed (or was cancelled) can be retried without re-downloading.
#[server]
pub async fn start_rescore(book_id: i64, slug: String) -> Result<String, ServerFnError> {
    let slug = crate::app::sanitize_slug(&slug);
    if slug.is_empty() {
        return Err(ServerFnError::new("no slug"));
    }
    Ok(crate::jobs::start_module(
        "rescore",
        "ingest.import_book",
        book_id,
        "",
        &format!("scoring {slug}…"),
        vec!["--rescore".to_string(), slug],
    ))
}

/// Every live/recent background job, for the queue panel.
#[server]
pub async fn queue_status() -> Result<Vec<JobProgress>, ServerFnError> {
    Ok(crate::jobs::list())
}

/// The panic button: cancel every catalog/grab/scoring job (tree-killing the running
/// one). Other kinds — OCR, reingest — belong to the books page and are left alone.
#[server]
pub async fn stop_queue() -> Result<(), ServerFnError> {
    for kind in ["catalog", "grab", "rescore"] {
        crate::jobs::cancel_all(kind);
    }
    Ok(())
}

// -------------------------------------------------------------------- page ---- #

/// The ingress prefix, read out of context exactly as app.rs's own `base_path()`
/// does. Duplicated (one line) because that one is private and this module isn't
/// allowed to widen it; fold the two together if it ever becomes `pub(crate)`.
fn base_path() -> String {
    use_context::<crate::app::BasePath>().map(|b| b.0).unwrap_or_default()
}

/// Results per page. 48 matches Standard Ebooks' own `per-page` and fills a screen
/// without making "select all on this page" a reckless button.
const PER_PAGE: i64 = 48;

/// Is this job one of ours (catalog sync / grab / scoring) rather than the books
/// page's OCR work? The queue panel shows everything, but only ours count as "live"
/// for the purposes of re-running the search when the queue drains.
fn is_live(j: &JobProgress) -> bool {
    matches!(j.status.as_str(), "queued" | "running")
}

/// `/get` — browse the synced catalogs, select titles, and bulk-download them.
#[component]
pub fn GetBooksPage() -> impl IntoView {
    // ---- filters. `draft` is what the box holds; `query` is what we've committed
    // to searching for, so typing doesn't fire a server fn per keystroke.
    let draft = RwSignal::new(String::new());
    let query = RwSignal::new(String::new());
    let source = RwSignal::new(String::new()); // "" = all sources
    let subject = RwSignal::new(String::new());
    let sort = RwSignal::new("relevance".to_string());
    let page = RwSignal::new(0i64);

    // Selection is keyed by identity, not by row index, so it survives paging and
    // re-searching within the session.
    let selected = RwSignal::new(HashSet::<(String, String)>::new());
    // The selectable (not-yet-imported) keys on the current page, for "select all".
    let page_keys = RwSignal::new(Vec::<(String, String)>::new());
    let total = RwSignal::new(0i64);

    // Bumped to re-run the search + status: after a sync, after a grab drains.
    let refresh = RwSignal::new(0u32);
    let notice = RwSignal::new(None::<String>);
    let err = RwSignal::new(None::<String>);

    let status = Resource::new(move || refresh.get(), |_| catalog_status());
    let subjects = Resource::new(
        move || (source.get(), refresh.get()),
        |(src, _)| catalog_subjects(src),
    );
    let results = Resource::new(
        move || {
            (
                query.get(),
                source.get(),
                subject.get(),
                sort.get(),
                page.get(),
                refresh.get(),
            )
        },
        |(q, src, subj, srt, pg, _)| search_catalog(q, src, subj, srt, PER_PAGE, pg * PER_PAGE),
    );

    // Keep the paging + "select all" helpers in sync with whatever just loaded.
    Effect::new(move |_| {
        if let Some(Ok(p)) = results.get() {
            total.set(p.total);
            page_keys.set(
                p.items
                    .iter()
                    .filter(|e| e.imported_slug.is_none())
                    .map(|e| (e.source.clone(), e.source_id.clone()))
                    .collect(),
            );
        }
    });

    // ---- the queue.
    let jobs = RwSignal::new(Vec::<JobProgress>::new());
    let live = RwSignal::new(false);
    let poll = move || {
        leptos::task::spawn_local(async move {
            if let Ok(list) = queue_status().await {
                let now_live = list.iter().any(is_live);
                // The moment the last job drains, re-run the search so the rows we
                // just downloaded flip to "already imported".
                if live.get_untracked() && !now_live {
                    refresh.update(|n| *n += 1);
                }
                live.set(now_live);
                jobs.set(list);
            }
        });
    };
    Effect::new(move |_| {
        poll(); // client-only; show any job already running when the page opens
        let tick = RwSignal::new(0u32);
        let h = leptos::prelude::set_interval_with_handle(
            move || {
                tick.update(|n| *n += 1);
                // Fast while something is live. Still ticking (slowly) when idle,
                // because grab_books queues the per-book scoring jobs server-side a
                // moment AFTER the grab finishes — we'd otherwise never see them.
                if live.get_untracked() || tick.get_untracked() % 5 == 0 {
                    poll();
                }
            },
            std::time::Duration::from_millis(1200),
        );
        if let Ok(h) = h {
            on_cleanup(move || h.clear());
        }
    });

    // ---- actions.
    let run_search = move || {
        page.set(0);
        query.set(draft.get_untracked());
    };
    // Search as you type, but only once you pause: a keystroke-per-query would run a
    // leading-wildcard LIKE over 60k rows on every letter of "middlemarch".
    let debounce = RwSignal::new(None::<leptos::prelude::TimeoutHandle>);
    let on_type = move |ev| {
        draft.set(event_target_value(&ev));
        if let Some(h) = debounce.get_untracked() {
            h.clear();
        }
        debounce.set(
            leptos::prelude::set_timeout_with_handle(
                move || run_search(),
                std::time::Duration::from_millis(350),
            )
            .ok(),
        );
    };
    let do_sync = move |src: String| {
        err.set(None);
        leptos::task::spawn_local(async move {
            match sync_catalog(src).await {
                Ok(_) => {
                    live.set(true);
                    poll();
                }
                Err(e) => err.set(Some(e.to_string())),
            }
        });
    };
    let do_grab = move |_| {
        let mut items: Vec<(String, String)> = selected.get_untracked().into_iter().collect();
        if items.is_empty() {
            return;
        }
        items.sort();
        let n = items.len();
        err.set(None);
        notice.set(None);
        leptos::task::spawn_local(async move {
            match grab_books(items).await {
                Ok(_) => {
                    selected.set(HashSet::new());
                    notice.set(Some(format!(
                        "queued {n} book{} — they'll appear in your library as they land, \
                         then each is scored in turn.",
                        if n == 1 { "" } else { "s" }
                    )));
                    live.set(true);
                    poll();
                }
                Err(e) => err.set(Some(e.to_string())),
            }
        });
    };
    let stop_all = move |_| {
        leptos::task::spawn_local(async move {
            let _ = stop_queue().await;
            poll();
        });
    };

    view! {
        <h1>"Get books"</h1>
        <p class="sub">
            "Search the Standard Ebooks and Project Gutenberg catalogs (synced locally, so this "
            "never touches the network), tick what you want, and download the lot. Books land in "
            "your library first and are scored one at a time afterwards."
        </p>

        // ---- sync / freshness row
        <Suspense fallback=move || view! { <p class="loading">"Checking catalogs…"</p> }>
            {move || status.get().map(|res| match res {
                Err(e) => view! { <p class="err">{e.to_string()}</p> }.into_any(),
                Ok(sum) => {
                    let never = sum.is_empty();
                    view! {
                        {never.then(|| view! {
                            <p class="cat-never">
                                "No catalogs synced yet — pull one down first. Standard Ebooks is "
                                "~1,500 carefully produced books and takes about a minute; "
                                "Gutenberg is ~60,000 rows and takes seconds."
                            </p>
                        })}
                        <div class="bar cat-syncbar">
                            {sum.sources.into_iter().map(|s| {
                                let src = s.source.clone();
                                let line = if s.rows == 0 {
                                    format!("{}: never synced", s.label)
                                } else if s.synced.is_empty() {
                                    format!("{}: {} books · {} imported", s.label, commas(s.rows), commas(s.imported))
                                } else {
                                    format!("{}: {} books · {} imported · synced {}",
                                        s.label, commas(s.rows), commas(s.imported), s.synced)
                                };
                                let cls = if s.rows == 0 { "chip primary" } else { "chip" };
                                view! {
                                    <span class="cat-srcstat">
                                        <span class="counts">{line}</span>
                                        <button type="button" class=cls
                                            on:click=move |_| do_sync(src.clone())>"sync now"</button>
                                    </span>
                                }
                            }).collect_view()}
                        </div>
                    }.into_any()
                }
            })}
        </Suspense>

        // ---- search / filter bar
        <div class="bar cat-bar">
            <select class="catsel" prop:value=move || source.get()
                on:change=move |e| { page.set(0); source.set(event_target_value(&e)); subject.set(String::new()); }>
                <option value="">"all sources"</option>
                <option value="standardebooks">"Standard Ebooks"</option>
                <option value="gutenberg">"Project Gutenberg"</option>
            </select>

            <input class="cat-q" type="search" placeholder="title or author…"
                prop:value=move || draft.get()
                on:input=on_type
                on:change=move |_| run_search()
                on:keydown=move |e: web_sys::KeyboardEvent| if e.key() == "Enter" { run_search() }/>
            <button type="button" class="chip" on:click=move |_| run_search()>"search"</button>

            <Suspense fallback=move || view! { <span class="loading">"…"</span> }>
                {move || subjects.get().and_then(|r| r.ok()).map(|subs| view! {
                    <select class="catsel" prop:value=move || subject.get()
                        on:change=move |e| { page.set(0); subject.set(event_target_value(&e)); }>
                        <option value="">"any subject"</option>
                        {subs.into_iter().map(|(name, n)| {
                            let v = name.clone();
                            view! { <option value=v>{format!("{name} ({n})")}</option> }
                        }).collect_view()}
                    </select>
                })}
            </Suspense>

            <select class="catsel" prop:value=move || sort.get()
                on:change=move |e| { page.set(0); sort.set(event_target_value(&e)); }>
                <option value="relevance">"best match"</option>
                <option value="title">"title"</option>
                <option value="author">"author"</option>
                <option value="year">"year"</option>
                <option value="words">"longest"</option>
            </select>
        </div>

        // ---- selection + download
        <div class="bar cat-selbar">
            <span class="counts">
                {move || {
                    let n = selected.get().len();
                    let t = total.get();
                    if n == 0 { format!("{} result{}", commas(t), if t == 1 { "" } else { "s" }) }
                    else { format!("{n} selected · {} result{}", commas(t), if t == 1 { "" } else { "s" }) }
                }}
            </span>
            <button type="button" class="chip"
                on:click=move |_| selected.update(|s| { for k in page_keys.get_untracked() { s.insert(k); } })>
                "select all on this page"
            </button>
            <button type="button" class="chip" disabled=move || selected.get().is_empty()
                on:click=move |_| selected.set(HashSet::new())>"clear selection"</button>
            <button type="button" class="chip primary cat-grab"
                disabled=move || selected.get().is_empty()
                on:click=do_grab>
                {move || {
                    let n = selected.get().len();
                    if n == 0 { "download".to_string() }
                    else { format!("download {n} book{}", if n == 1 { "" } else { "s" }) }
                }}
            </button>
        </div>
        {move || notice.get().map(|m| view! { <p class="counts cat-notice">{m}</p> })}
        {move || err.get().map(|e| view! { <p class="err">{e}</p> })}

        // ---- the queue
        {move || {
            let list = jobs.get();
            (!list.is_empty()).then(|| view! {
                <div class="q-panel">
                    <div class="q-head">
                        <span class="counts">
                            {format!("queue · {} job{}", list.len(), if list.len() == 1 { "" } else { "s" })}
                        </span>
                        <button type="button" class="chip" on:click=stop_all>"stop all"</button>
                    </div>
                    <ul class="q-list">
                        {list.into_iter().map(|j| {
                            let id = j.id.clone();
                            let pct = j.percent;
                            let running = is_live(&j);
                            let cancel = move |_| {
                                let id = id.clone();
                                leptos::task::spawn_local(async move {
                                    let _ = crate::app::cancel_job(id).await;
                                });
                            };
                            view! {
                                <li class="q-job" class:done=!running>
                                    <span class="q-kind">{j.kind.clone()}</span>
                                    <span class="q-msg">{j.message.clone()}</span>
                                    {(running && pct >= 0.0).then(|| view! {
                                        <progress class="jobprog" max="100" value=pct></progress>
                                    })}
                                    // The status word is also a class (`s-failed`, …)
                                    // so CSS can colour it — text content isn't selectable.
                                    <span class=format!("q-status s-{}", j.status)>{j.status.clone()}</span>
                                    {running.then(|| view! {
                                        <button type="button" class="chip" on:click=cancel>"cancel"</button>
                                    })}
                                </li>
                            }
                        }).collect_view()}
                    </ul>
                </div>
            })
        }}

        // ---- results
        <Suspense fallback=move || view! { <p class="loading">"Searching…"</p> }>
            {move || results.get().map(|res| match res {
                Err(e) => view! { <p class="err">{e.to_string()}</p> }.into_any(),
                Ok(p) if p.items.is_empty() => view! {
                    <p class="counts">"No matches."</p>
                }.into_any(),
                Ok(p) => view! {
                    <ul class="cat-list">
                        {p.items.into_iter().map(|e| {
                            let key = (e.source.clone(), e.source_id.clone());
                            let k_read = key.clone();
                            let imported = e.imported_slug.clone();
                            let is_imported = imported.is_some();
                            let toggle = move |ev| {
                                let on = event_target_checked(&ev);
                                let k = key.clone();
                                selected.update(|s| { if on { s.insert(k); } else { s.remove(&k); } });
                            };
                            // year · words · reading ease · source, skipping whatever
                            // this source doesn't report.
                            let mut bits: Vec<String> = Vec::new();
                            if let Some(y) = e.year { bits.push(y.to_string()); }
                            if let Some(w) = e.n_words { bits.push(format!("{} words", commas(w))); }
                            if let Some(r) = e.reading_ease { bits.push(format!("{r:.0} ease")); }
                            bits.push(source_label(&e.source).to_string());
                            let meta = bits.join(" · ");
                            let subs = e.subjects.clone();
                            let book_href = e.imported_book_id
                                .map(|id| format!("{}/?book={id}", base_path()));
                            view! {
                                <li class=if is_imported { "cat-row cat-have" } else { "cat-row" }>
                                    <input type="checkbox" class="cat-pick"
                                        disabled=is_imported
                                        prop:checked=move || selected.with(|s| s.contains(&k_read))
                                        on:change=toggle/>
                                    <div class="cat-main">
                                        <span class="cat-title">{e.title.clone()}</span>
                                        <span class="cat-author">{e.author.clone()}</span>
                                        <span class="counts cat-meta">{meta}</span>
                                        <div class="cat-subs">
                                            {subs.into_iter().take(4).map(|s|
                                                view! { <span class="catchip">{s}</span> }
                                            ).collect_view()}
                                        </div>
                                    </div>
                                    {book_href.map(|href| view! {
                                        <a class="cat-done reltgt" href=href>"already imported"</a>
                                    })}
                                </li>
                            }
                        }).collect_view()}
                    </ul>
                }.into_any(),
            })}
        </Suspense>

        // ---- paging
        {move || {
            let t = total.get();
            let pg = page.get();
            let last = if t <= 0 { 0 } else { (t - 1) / PER_PAGE };
            (t > PER_PAGE).then(|| view! {
                <div class="bar cat-pager">
                    <button type="button" class="chip" disabled=pg <= 0
                        on:click=move |_| page.update(|p| *p = (*p - 1).max(0))>"‹ prev"</button>
                    <span class="counts">{format!("page {} of {}", pg + 1, last + 1)}</span>
                    <button type="button" class="chip" disabled=pg >= last
                        on:click=move |_| page.update(|p| *p = (*p + 1).min(last))>"next ›"</button>
                </div>
            })
        }}
    }
}
