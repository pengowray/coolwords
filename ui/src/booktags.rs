//! Book-level tags: a library-organising layer above the per-word tags in
//! `user.db`. Books get named tags so the books page and the book dropdown can
//! group and filter a growing library.
//!
//! Two flavours share the one `u.book_tags` table:
//!
//! * **auto** (`auto = 1`) — derived from the book itself (`books.source` →
//!   `src:<slug>`, `books.format` → `fmt:<slug>`) and RE-RECONCILED ON EVERY READ.
//!   That's deliberate: books imported before this feature existed pick their auto
//!   tags up with no migration, a re-import that changes a book's format fixes
//!   itself, and there is no way for the derived state to drift from the books
//!   table. The user can't delete one (the next read would put it back), so the UI
//!   renders them muted with no ✕.
//! * **manual** (`auto = 0`) — free text the user types, normalised by
//!   [`sanitize_book_tag`]. The `src:` / `fmt:` namespaces are reserved.
//!
//! The joins need `books` (coolwords.db) AND `book_tags` (user.db), so everything
//! here goes through [`crate::app::open_conn`], which ATTACHes the user DB as `u`.
//!
//! Types + the normaliser are deliberately NOT behind `cfg(ssr)` — the hydrated
//! client deserializes the types and re-runs the normaliser so its optimistic chip
//! matches what the server will store.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[cfg(feature = "ssr")]
use std::collections::{HashMap, HashSet};

/// A book tag, with the count of books carrying it (for the sidebar's tag list).
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct BookTag {
    pub name: String,
    /// Derived from the book's source/format rather than applied by hand — shown
    /// muted, not removable, and regenerated on every read.
    pub auto: bool,
    /// How many (existing) books carry this tag, collection-wide. Filled in on both
    /// `list_book_tags` and `books_with_tags`, so a per-book chip can still show
    /// "17 books have this".
    pub n_books: i64,
}

/// A book plus its tags, as rendered by the books page and the book dropdown.
///
/// Deliberately does NOT carry the ★ (`n_selected`) count: computing it needs the
/// big correlated subquery that `list_books` / `list_books_admin` already run, and
/// duplicating that SQL a third time is exactly the drift risk we don't want. Join
/// this to `list_books()` / `list_books_admin()` on `book_id` instead — both are
/// already fetched by the pages that want tags.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct BookWithTags {
    pub book_id: i64,
    pub slug: String,
    pub title: String,
    /// Auto tags first, then alphabetical — the same order `list_book_tags` uses.
    #[serde(default)]
    pub tags: Vec<BookTag>,
}

// ---- normalisation (pure; runs on both client and server) ---- #

/// The two namespaces this module owns. A manual tag may not live in either.
pub const RESERVED_PREFIXES: [&str; 2] = ["src", "fmt"];

/// Normalize a free-text book tag to its canonical stored form, or None if it isn't
/// usable. Pure (client + server) so the optimistic chip matches storage.
///
/// A book tag is `head` or `head:tail` (at most one `:`, which is what the auto
/// prefixes use for `src:gutenberg` / `fmt:epub`). Each side is sanitized by the
/// word-tag normaliser [`crate::app::sanitize_tag`] — lowercased, `[a-z0-9 -]`,
/// 1..30 chars per dotted segment, at most 4 dotted levels, at least one letter —
/// so a `.` nests a book tag under its prefix exactly like it does for word tags.
/// Whole thing capped at 60 chars.
///
/// Inherited quirk worth knowing: `sanitize_tag` reserves `pick` as a head segment
/// (it's the word tags' contextual-bucket namespace), so a book tag can't be called
/// `pick` either. Harmless, and not worth a second near-identical normaliser.
pub fn sanitize_book_tag(name: &str) -> Option<String> {
    let lowered = name.trim().to_lowercase();
    if lowered.matches(':').count() > 1 {
        return None;
    }
    let clean = match lowered.split_once(':') {
        None => crate::app::sanitize_tag(&lowered)?,
        Some((head, tail)) => {
            format!("{}:{}", crate::app::sanitize_tag(head)?, crate::app::sanitize_tag(tail)?)
        }
    };
    (clean.chars().count() <= 60).then_some(clean)
}

/// Is this name in a namespace the reconciler owns? Both separators are refused
/// (`src:x` and `src.x`) — the schema comment describes the derived tags with a
/// dot and the code uses a colon, and neither should be typeable by hand. The bare
/// heads `src` / `fmt` are refused too, so the hierarchy roots stay ours.
pub fn is_reserved_book_tag(name: &str) -> bool {
    RESERVED_PREFIXES.iter().any(|p| {
        name == *p
            || name.strip_prefix(p).is_some_and(|r| r.starts_with(':') || r.starts_with('.'))
    })
}

// ---- auto-tag derivation + reconcile ---- #

/// The `src:` tag for a `books.source` value.
///
/// Hand-dropped files land with source 'import' / 'epub' / 'txt' / 'pdf' / '' —
/// which import path created them is an implementation detail, and they all mean
/// the same thing to a reader ("I supplied this file myself"), so they collapse to
/// `src:upload`. The catalog writes 'standardebooks'; hyphenate it for reading.
#[cfg(feature = "ssr")]
fn src_auto_tag(source: &str) -> String {
    let s = source.trim().to_lowercase();
    let slug = match s.as_str() {
        "" | "import" | "upload" | "file" | "epub" | "txt" | "pdf" => "upload".to_string(),
        "standardebooks" | "standard ebooks" | "se" => "standard-ebooks".to_string(),
        other => crate::app::sanitize_slug(other),
    };
    let tag = format!("src:{slug}");
    // Every stored tag must be canonical (rename/delete guards assume it). A source
    // that sanitizes to junk — all digits, punctuation only — falls back rather than
    // poisoning the table with something no normaliser will accept later.
    if sanitize_book_tag(&tag).as_deref() == Some(tag.as_str()) {
        tag
    } else {
        "src:upload".to_string()
    }
}

/// The `fmt:` tag for a `books.format` value, or None when the book has no format
/// recorded (nothing sensible to derive — better no chip than `fmt:unknown`).
#[cfg(feature = "ssr")]
fn fmt_auto_tag(format: &str) -> Option<String> {
    let slug = crate::app::sanitize_slug(&format.trim().to_lowercase());
    if slug.is_empty() {
        return None;
    }
    let tag = format!("fmt:{slug}");
    (sanitize_book_tag(&tag).as_deref() == Some(tag.as_str())).then_some(tag)
}

/// Bring the `auto = 1` rows in line with the books table. Called by every read
/// path, so it is written to do NOTHING (two indexed scans, no transaction, no
/// write) in the steady state — there are dozens of books, not thousands.
///
/// Only slugs that currently exist in `books` are reconciled. An auto row whose
/// book was deleted is left dormant, matching the contract the rest of user.db
/// keeps ("deleting a book does not delete its tags; they reattach if the same slug
/// comes back") — and if the slug does come back with a different source/format,
/// this same pass corrects it then.
#[cfg(feature = "ssr")]
pub(crate) fn reconcile_book_tags(conn: &rusqlite::Connection) -> Result<(), ServerFnError> {
    let err = |e: rusqlite::Error| ServerFnError::new(e.to_string());

    let mut want: HashSet<(String, String)> = HashSet::new();
    let mut known: HashSet<String> = HashSet::new();
    {
        let mut stmt = conn
            .prepare("SELECT slug, COALESCE(source,''), COALESCE(format,'') FROM books")
            .map_err(err)?;
        let rows = stmt
            .query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
            })
            .map_err(err)?;
        for (slug, source, format) in rows.filter_map(Result::ok) {
            want.insert((slug.clone(), src_auto_tag(&source)));
            if let Some(f) = fmt_auto_tag(&format) {
                want.insert((slug.clone(), f));
            }
            known.insert(slug);
        }
    }

    let mut have: HashSet<(String, String)> = HashSet::new();
    {
        let mut stmt = conn
            .prepare("SELECT book_slug, tag FROM u.book_tags WHERE auto = 1")
            .map_err(err)?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(err)?;
        have.extend(rows.filter_map(Result::ok));
    }

    let stale: Vec<&(String, String)> =
        have.iter().filter(|p| known.contains(&p.0) && !want.contains(*p)).collect();
    let missing: Vec<&(String, String)> = want.difference(&have).collect();
    if stale.is_empty() && missing.is_empty() {
        return Ok(()); // the overwhelmingly common case: nothing to do, nothing written
    }

    // `conn` is shared (&, not &mut) so we can't use rusqlite's Transaction guard;
    // drive BEGIN/COMMIT by hand and make sure an error can't strand an open txn.
    conn.execute_batch("BEGIN IMMEDIATE").map_err(err)?;
    let apply = || -> rusqlite::Result<()> {
        {
            let mut del = conn.prepare(
                "DELETE FROM u.book_tags WHERE book_slug = ?1 AND tag = ?2 AND auto = 1",
            )?;
            for (s, t) in &stale {
                del.execute(rusqlite::params![s, t])?;
            }
        }
        let mut ins = conn.prepare(
            "INSERT OR IGNORE INTO u.book_tags(book_slug, tag, auto, ts)
             VALUES (?1, ?2, 1, datetime('now'))",
        )?;
        for (s, t) in &missing {
            ins.execute(rusqlite::params![s, t])?;
        }
        Ok(())
    };
    let res = apply();
    let _ = conn.execute_batch(if res.is_ok() { "COMMIT" } else { "ROLLBACK" });
    res.map_err(err)?;
    Ok(())
}

// ---- server fns ---- #

/// Every distinct book tag with how many books carry it, auto tags first then
/// alphabetical. Reconciles the auto rows first, so this is the call that makes a
/// freshly-imported (or pre-feature) book grow its `src:` / `fmt:` chips.
#[server]
pub async fn list_book_tags() -> Result<Vec<BookTag>, ServerFnError> {
    let conn = crate::app::open_conn()?;
    reconcile_book_tags(&conn)?;
    let mut stmt = conn
        .prepare(
            // JOIN books so tags left dormant by a deleted book don't show up with a
            // count that no longer means anything.
            "SELECT bt.tag, MAX(bt.auto), count(*)
             FROM u.book_tags bt JOIN books b ON b.slug = bt.book_slug
             GROUP BY bt.tag ORDER BY MAX(bt.auto) DESC, bt.tag",
        )
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let out = stmt
        .query_map([], |r| {
            Ok(BookTag {
                name: r.get(0)?,
                // MAX(): a name is "auto" if any row for it is. Manual rows can't use
                // the reserved namespaces, so in practice the two never mix.
                auto: r.get::<_, i64>(1)? != 0,
                n_books: r.get(2)?,
            })
        })
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .filter_map(Result::ok)
        .collect();
    Ok(out)
}

/// Every book with its resolved tag list — the books page's chip rows, and the
/// source of the book dropdown's optgroups. Two queries, no per-row subqueries.
#[server]
pub async fn books_with_tags() -> Result<Vec<BookWithTags>, ServerFnError> {
    let conn = crate::app::open_conn()?;
    reconcile_book_tags(&conn)?;

    let mut out: Vec<BookWithTags> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    {
        let mut stmt = conn
            .prepare("SELECT id, slug, COALESCE(title, slug) FROM books ORDER BY id")
            .map_err(|e| ServerFnError::new(e.to_string()))?;
        let rows = stmt
            .query_map([], |r| {
                Ok(BookWithTags {
                    book_id: r.get(0)?,
                    slug: r.get(1)?,
                    title: r.get(2)?,
                    tags: Vec::new(),
                })
            })
            .map_err(|e| ServerFnError::new(e.to_string()))?;
        for b in rows.filter_map(Result::ok) {
            at.insert(b.slug.clone(), out.len());
            out.push(b);
        }
    }

    let mut stmt = conn
        .prepare(
            // The CTE gives every chip its collection-wide book count in one pass,
            // instead of a correlated subquery per (book, tag) row.
            "WITH counts AS (
               SELECT bt.tag AS tag, count(*) AS n
               FROM u.book_tags bt JOIN books b ON b.slug = bt.book_slug
               GROUP BY bt.tag)
             SELECT bt.book_slug, bt.tag, bt.auto, c.n
             FROM u.book_tags bt
               JOIN books b ON b.slug = bt.book_slug
               JOIN counts c ON c.tag = bt.tag
             ORDER BY bt.auto DESC, bt.tag",
        )
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                BookTag {
                    name: r.get(1)?,
                    auto: r.get::<_, i64>(2)? != 0,
                    n_books: r.get(3)?,
                },
            ))
        })
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    for (slug, tag) in rows.filter_map(Result::ok) {
        if let Some(&i) = at.get(&slug) {
            out[i].tags.push(tag);
        }
    }
    Ok(out)
}

/// Apply a manual tag to one book. Normalises, refuses the reserved namespaces,
/// and returns the canonical name so the client can settle its optimistic chip.
#[server]
pub async fn add_book_tag(book_slug: String, tag: String) -> Result<String, ServerFnError> {
    use rusqlite::OptionalExtension;
    let clean = sanitize_book_tag(&tag).ok_or_else(|| ServerFnError::new("invalid tag name"))?;
    if is_reserved_book_tag(&clean) {
        return Err(ServerFnError::new(
            "‘src:’ and ‘fmt:’ tags are derived from the book itself — pick another name",
        ));
    }
    let conn = crate::app::open_conn()?;
    // Tag rows are keyed by slug with no foreign key, so a typo'd slug would create a
    // dormant row that never shows up anywhere. Fail loudly instead.
    let exists = conn
        .query_row("SELECT 1 FROM books WHERE slug = ?1", [&book_slug], |_| Ok(()))
        .optional()
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .is_some();
    if !exists {
        return Err(ServerFnError::new("unknown book"));
    }
    conn.execute(
        "INSERT OR IGNORE INTO u.book_tags(book_slug, tag, auto, ts)
         VALUES (?1, ?2, 0, datetime('now'))",
        rusqlite::params![book_slug, clean],
    )
    .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(clean)
}

/// Remove a manual tag from one book. Idempotent (removing a tag that isn't there
/// succeeds), but refuses an auto row rather than silently no-op'ing on one.
#[server]
pub async fn remove_book_tag(book_slug: String, tag: String) -> Result<(), ServerFnError> {
    use rusqlite::OptionalExtension;
    let clean = sanitize_book_tag(&tag).ok_or_else(|| ServerFnError::new("invalid tag name"))?;
    let conn = crate::app::open_conn()?;
    let n = conn
        .execute(
            "DELETE FROM u.book_tags WHERE book_slug = ?1 AND tag = ?2 AND auto = 0",
            rusqlite::params![book_slug, clean],
        )
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    if n == 0 {
        let is_auto = conn
            .query_row(
                "SELECT 1 FROM u.book_tags WHERE book_slug = ?1 AND tag = ?2 AND auto = 1",
                rusqlite::params![book_slug, clean],
                |_| Ok(()),
            )
            .optional()
            .map_err(|e| ServerFnError::new(e.to_string()))?
            .is_some();
        if is_auto {
            return Err(ServerFnError::new(
                "that tag comes from the book's source/format — it can't be removed",
            ));
        }
    }
    Ok(())
}

/// Rename a manual tag across EVERY book, cascading to its dotted descendants
/// (`shelf` → `stack` also moves `shelf.poetry` → `stack.poetry`). Books that
/// already carry the new name just absorb the move. Returns the canonical new name.
#[server]
pub async fn rename_book_tag(old: String, new: String) -> Result<String, ServerFnError> {
    use rusqlite::OptionalExtension;
    // `old` is interpolated into a LIKE subtree pattern below and this is a public
    // endpoint, so it must already be canonical (which blocks `%`/`_` injection).
    if sanitize_book_tag(&old).as_deref() != Some(old.as_str()) {
        return Err(ServerFnError::new("invalid tag name"));
    }
    let clean = sanitize_book_tag(&new).ok_or_else(|| ServerFnError::new("invalid tag name"))?;
    if clean == old {
        return Ok(clean);
    }
    if is_reserved_book_tag(&old) || is_reserved_book_tag(&clean) {
        return Err(ServerFnError::new("‘src:’ and ‘fmt:’ tags are derived — they can't be renamed"));
    }
    if crate::app::is_ancestor(&old, &clean) {
        return Err(ServerFnError::new("can't rename a tag under itself"));
    }
    let conn = crate::app::open_conn()?;
    // Belt and braces: the reserved-name check above should already make this
    // impossible, but never move a row the reconciler owns.
    let touches_auto = conn
        .query_row(
            "SELECT 1 FROM u.book_tags WHERE (tag = ?1 OR tag LIKE ?1 || '.%') AND auto = 1",
            [&old],
            |_| Ok(()),
        )
        .optional()
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .is_some();
    if touches_auto {
        return Err(ServerFnError::new("that tag is maintained automatically"));
    }
    // Copy-then-delete rather than UPDATE: the PK is (book_slug, tag), so a book that
    // already carries the destination name would make a straight UPDATE fail. INSERT
    // OR IGNORE merges instead.
    conn.execute(
        "INSERT OR IGNORE INTO u.book_tags(book_slug, tag, auto, ts)
         SELECT book_slug, ?1 || substr(tag, ?3), 0, ts
         FROM u.book_tags WHERE (tag = ?2 OR tag LIKE ?2 || '.%') AND auto = 0",
        rusqlite::params![clean, old, old.len() as i64 + 1],
    )
    .map_err(|e| ServerFnError::new(e.to_string()))?;
    conn.execute(
        "DELETE FROM u.book_tags WHERE (tag = ?1 OR tag LIKE ?1 || '.%') AND auto = 0",
        [&old],
    )
    .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(clean)
}

/// Delete a manual tag (and its dotted descendants) from every book; returns how
/// many rows went. Refuses the derived namespaces.
#[server]
pub async fn delete_book_tag(name: String) -> Result<i64, ServerFnError> {
    // Canonical-only, same LIKE-injection reasoning as rename_book_tag.
    if sanitize_book_tag(&name).as_deref() != Some(name.as_str()) {
        return Err(ServerFnError::new("invalid tag name"));
    }
    if is_reserved_book_tag(&name) {
        return Err(ServerFnError::new("‘src:’ and ‘fmt:’ tags are derived — they can't be deleted"));
    }
    let conn = crate::app::open_conn()?;
    let n = conn
        .execute(
            "DELETE FROM u.book_tags WHERE (tag = ?1 OR tag LIKE ?1 || '.%') AND auto = 0",
            [&name],
        )
        .map_err(|e| ServerFnError::new(e.to_string()))? as i64;
    Ok(n)
}

// ---- the chip row (used inside /books' BookCard) ---- #

/// A book's tag chips plus a "+ tag" input. Auto chips are muted and have no ✕;
/// manual chips get one. Adding/removing updates the local chip list immediately
/// and then fires `on_change` so the page can refetch the authoritative list.
///
/// `on_change` is optional so a caller that re-renders on its own can skip it, but
/// pass it if you have a Resource to refresh — without it a failed write leaves the
/// optimistic chip on screen (with the error text beside it) until the next load.
#[component]
pub fn BookTagEditor(
    book_id: i64,
    slug: String,
    tags: Vec<BookTag>,
    #[prop(optional)] on_change: Option<Callback<()>>,
) -> impl IntoView {
    let chips = RwSignal::new(tags);
    let draft = RwSignal::new(String::new());
    let err = RwSignal::new(String::new());
    // The slug never changes for a given card; StoredValue keeps it cheaply cloneable
    // into the async closures without making them capture a String each.
    let slug = StoredValue::new(slug);
    let notify = move || {
        if let Some(cb) = on_change {
            cb.run(());
        }
    };

    let add = move || {
        let raw = draft.get();
        if raw.trim().is_empty() {
            return;
        }
        let Some(clean) = sanitize_book_tag(&raw) else {
            err.set("that isn't a usable tag name".to_string());
            return;
        };
        if is_reserved_book_tag(&clean) {
            err.set("‘src:’ and ‘fmt:’ tags come from the book itself".to_string());
            return;
        }
        draft.set(String::new());
        if chips.with(|c| c.iter().any(|t| t.name == clean)) {
            err.set(String::new());
            return; // already on this book — typing it again is a no-op, not an error
        }
        err.set(String::new());
        chips.update(|c| c.push(BookTag { name: clean.clone(), auto: false, n_books: 0 }));
        let sent = clean.clone();
        leptos::task::spawn_local(async move {
            match add_book_tag(slug.get_value(), sent.clone()).await {
                Ok(canon) => {
                    // The server's normaliser is the same pure fn, so this only differs
                    // in weird cases — but settle on the stored name regardless.
                    if canon != sent {
                        chips.update(|c| {
                            for t in c.iter_mut() {
                                if t.name == sent {
                                    t.name = canon.clone();
                                }
                            }
                        });
                    }
                    notify();
                }
                Err(e) => {
                    chips.update(|c| c.retain(|t| t.name != sent));
                    err.set(e.to_string());
                }
            }
        });
    };

    let remove = move |name: String| {
        chips.update(|c| c.retain(|t| t.name != name));
        err.set(String::new());
        leptos::task::spawn_local(async move {
            if let Err(e) = remove_book_tag(slug.get_value(), name).await {
                err.set(e.to_string());
            }
            notify();
        });
    };

    view! {
        <div class="bt-tags">
            {move || chips.get().into_iter().map(|t| {
                let name = t.name.clone();
                let n = t.n_books;
                if t.auto {
                    let title = format!("derived from this book's source/format{}",
                        if n > 0 { format!(" · {n} books") } else { String::new() });
                    view! {
                        <span class="chip bt-auto" title=title>{name}</span>
                    }.into_any()
                } else {
                    let gone = name.clone();
                    let title = if n > 1 { format!("{n} books have this tag") } else { String::new() };
                    view! {
                        <span class="bt-chip">
                            <span class="chip bt-tag" title=title>{name}</span>
                            <button type="button" class="catx bt-x" title="remove tag"
                                on:click=move |_| remove(gone.clone())>"✕"</button>
                        </span>
                    }.into_any()
                }
            }).collect_view()}
            <input class="bt-add" id=format!("bt-add-{book_id}") placeholder="+ tag"
                prop:value=move || draft.get()
                on:input=move |ev| draft.set(event_target_value(&ev))
                on:keydown=move |ev| if ev.key() == "Enter" { add(); }
                on:blur=move |_| add()/>
            {move || {
                let e = err.get();
                (!e.is_empty()).then(|| view! { <span class="bt-err err">{e}</span> })
            }}
        </div>
    }
}
