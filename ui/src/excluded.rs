//! Why a book's words don't all reach the word list.
//!
//! `ingest/score.py` drops most of a book's vocabulary before anything is ranked:
//! inflected forms, proper nouns, words too common to be interesting, tokens the
//! dictionary has never heard of. That is invisible on the words page — you see
//! "400 shown" and no hint that 14,000 words were set aside, or which ones.
//!
//! This module reproduces those hard filters as one SQL `CASE`, in the same order
//! `score_group` applies them, so every distinct word in a book lands in exactly
//! one bucket: kept, or a named reason. The counts therefore add up to
//! `books.n_types` — if they ever stop adding up, [`REASONS`] and `score.py` have
//! drifted apart.
//!
//! # Keeping this in step with the scorer
//!
//! The `CASE` in `why_select` mirrors `ingest/score.py`:
//!
//! | score.py                                    | key           |
//! |---------------------------------------------|---------------|
//! | `occ` query: `o.word_id IS NOT NULL`        | `nodict`      |
//! | `occ` query: `w.alpha_only = 1`             | `nonalpha`    |
//! | `score_group`: `not inwikt`                 | `nowikt`      |
//! | `score_group`: `formof`                     | `formof`      |
//! | `score_group`: `proper`                     | `proper`      |
//! | `score_group`: `offensive`                  | `offensive`   |
//! | `score_group`: `cap > 0.5`                  | `capitalised` |
//! | `score_group`: `length < MIN_LEN`           | `short`       |
//! | `score_group`: `freq_comb > MAX_FREQ_PM_LC` | `common`      |
//!
//! The first five are a single `or` in Python, so the ORDER among them is this
//! module's editorial choice (most-explanatory first), not the scorer's.
//!
//! Everything is judged on the GROUP REPRESENTATIVE at the current merge level,
//! exactly as the scorer does — at level 2 "whales" is left out because "whale"
//! is too common, and the table says so by showing `whales → whale`.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

use crate::app::err_text;

/// Frequency threshold in `score.py` (`MAX_FREQ_PM_LC`), quoted in the UI.
const MAX_FREQ_PM_LC: f64 = 12.0;
/// Shortest word the scorer will consider (`score.py` `MIN_LEN`).
const MIN_LEN: i64 = 3;
/// Sort key putting words with no measured frequency last. `score.py` treats those
/// as the rarest of all, but in a book they are overwhelmingly names, typos and
/// contractions — leading with them buries every word whose rarity is a fact.
#[cfg(feature = "ssr")]
const UNMEASURED_LAST: &str = "(CASE WHEN pm IS NULL OR pm <= 0 THEN 1 ELSE 0 END)";
/// Rows returned for one reason. Enough to browse; not enough to freeze the page.
const WORD_LIMIT: i32 = 200;

/// The buckets, in the order they're tested. `key` is what the UI passes back to
/// [`excluded_words`]; `note` is the tooltip.
pub const REASONS: &[(&str, &str, &str)] = &[
    ("nodict", "not a dictionary word", "a name, a typo, a scanning error, or simply missing from the dictionary"),
    ("nonalpha", "not plain letters", "words with a hyphen or apostrophe, like \"man-of-war\" or \"don't\""),
    ("nowikt", "no Wiktionary entry", "in the dictionary, but with no Wiktionary entry to check it against"),
    ("formof", "form of another word", "a plural, tense or comparative. It counts towards its base word instead."),
    ("proper", "proper noun", "names of people, places and brands"),
    ("offensive", "offensive", "some sense of it is tagged as a slur, vulgar or derogatory"),
    ("capitalised", "usually capitalised", "over half its uses in print are capitalised, so it reads as a name"),
    ("short", "under 3 letters", "the scorer needs at least 3 letters"),
    ("common", "too common", "used more than 12 times per million words of general English"),
];

pub fn reason_label(key: &str) -> &'static str {
    REASONS.iter().find(|(k, _, _)| *k == key).map(|(_, l, _)| *l).unwrap_or("left out")
}

/// One bucket with its size for this book and merge level.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExcludeReason {
    pub key: String,
    pub label: String,
    pub note: String,
    pub count: i64,
}

/// The accounting for one book at one merge level. `n_kept + every reason count`
/// equals `n_types`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExcludeReport {
    /// Distinct words in the book (`books.n_types`).
    pub n_types: i64,
    /// Distinct words that survive into a scored group.
    pub n_kept: i64,
    /// Scored entries at this level — below `n_kept` wherever forms merged.
    pub n_candidates: i64,
    /// Distinct words left out, by reason, biggest bucket first.
    pub reasons: Vec<ExcludeReason>,
}

/// One left-out word.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExcludedWord {
    /// The form as it appears in the book.
    pub word: String,
    /// The merged form the filter actually judged, when it differs from `word`.
    pub head: Option<String>,
    /// Word id of the judged form, for opening its detail sheet. `None` when the
    /// token isn't a dictionary word at all.
    pub word_id: Option<i64>,
    /// Times this form appears in the book.
    pub count: i64,
    /// General-English frequency per million (lowercase uses), the number the
    /// "too common" test reads. `None` when the word isn't in the corpus.
    pub freq_pm: Option<f64>,
    pub gloss: Option<String>,
    pub reason: String,
}

/// One row per distinct word in the book, tagged with its reason key (`''` when it
/// reaches the ranking) and everything the table shows. `?1` = book id, `?2` = merge
/// level. The `CASE` order is the one documented at the top of this module.
///
/// `tw` is the token's own dictionary row, `rw` the group representative at this
/// level, `lf` the family's combined frequency — the three things `score.py` reads.
#[cfg(feature = "ssr")]
fn why_select() -> String {
    format!(
        "SELECT bo.token AS word, rw.word AS head, rw.id AS word_id, bo.count AS n,
                COALESCE(lf.freq_pm_lc, rw.freq_pm_lc) AS pm, rw.gloss AS gloss,
                CASE
                  WHEN bo.word_id IS NULL THEN 'nodict'
                  WHEN COALESCE(tw.alpha_only, 0) = 0 THEN 'nonalpha'
                  WHEN EXISTS (SELECT 1 FROM candidates c
                               WHERE c.book_id = ?1 AND c.level = ?2
                                 AND c.word_id = COALESCE(wl.lemma_id, bo.word_id)) THEN ''
                  WHEN COALESCE(rw.in_wiktionary, 0) = 0 THEN 'nowikt'
                  WHEN rw.is_form_of = 1 THEN 'formof'
                  WHEN rw.is_proper = 1 THEN 'proper'
                  WHEN rw.is_offensive = 1 THEN 'offensive'
                  WHEN rw.cap_ratio > 0.5 THEN 'capitalised'
                  WHEN COALESCE(rw.length, 0) < {MIN_LEN} THEN 'short'
                  WHEN COALESCE(lf.freq_pm_lc, rw.freq_pm_lc) > {MAX_FREQ_PM_LC} THEN 'common'
                  ELSE '' END AS why
         FROM book_occurrences bo
         LEFT JOIN word_lemma wl ON wl.word_id = bo.word_id AND wl.level = ?2
         LEFT JOIN words tw ON tw.id = bo.word_id
         LEFT JOIN words rw ON rw.id = COALESCE(wl.lemma_id, bo.word_id)
         LEFT JOIN lemma_freq lf ON lf.level = ?2 AND lf.lemma_id = COALESCE(wl.lemma_id, bo.word_id)
         WHERE bo.book_id = ?1"
    )
}

#[server]
pub async fn exclusion_report(book_id: i64, level: i64) -> Result<ExcludeReport, ServerFnError> {
    use std::collections::HashMap;
    let conn = rusqlite::Connection::open(crate::app::db_path())
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let sql = format!("SELECT why, count(*) FROM ({}) GROUP BY why", why_select());
    let mut stmt = conn.prepare(&sql).map_err(|e| ServerFnError::new(e.to_string()))?;
    let counts: HashMap<String, i64> = stmt
        .query_map([book_id, level], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .filter_map(Result::ok)
        .collect();

    let n_types: i64 = conn
        .query_row("SELECT COALESCE(n_types, 0) FROM books WHERE id = ?1", [book_id], |r| r.get(0))
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let n_candidates: i64 = conn
        .query_row(
            "SELECT count(*) FROM candidates WHERE book_id = ?1 AND level = ?2",
            [book_id, level],
            |r| r.get(0),
        )
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let mut reasons: Vec<ExcludeReason> = REASONS
        .iter()
        .filter_map(|(key, label, note)| {
            let count = *counts.get(*key)?;
            (count > 0).then(|| ExcludeReason {
                key: key.to_string(),
                label: label.to_string(),
                note: note.to_string(),
                count,
            })
        })
        .collect();
    reasons.sort_by(|a, b| b.count.cmp(&a.count));

    Ok(ExcludeReport {
        n_types,
        n_kept: counts.get("").copied().unwrap_or(0),
        n_candidates,
        reasons,
    })
}

/// The left-out words themselves. `reason` empty = every reason together;
/// `sort` is "rare" (default) or "count".
#[server]
pub async fn excluded_words(
    book_id: i64,
    level: i64,
    reason: String,
    sort: String,
) -> Result<Vec<ExcludedWord>, ServerFnError> {
    let conn = rusqlite::Connection::open(crate::app::db_path())
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    // Rarity ties break on "used most here", and vice versa: a word that is both
    // rare and frequent in this book is the one worth seeing first either way.
    let order = match sort.as_str() {
        "count" => format!("n DESC, {UNMEASURED_LAST} ASC, pm ASC"),
        _ => format!("{UNMEASURED_LAST} ASC, pm ASC, n DESC"),
    };
    // `reason` is compared against the CASE output as a bound parameter, so an
    // unknown value can only ever mean "no rows" — but fall back to "all" anyway,
    // which is what a stale chip in a URL should show.
    let want = if REASONS.iter().any(|(k, _, _)| *k == reason) { reason } else { String::new() };
    let sql = format!(
        "SELECT word, head, word_id, n, pm, gloss, why FROM ({})
         WHERE why <> '' AND (?3 = '' OR why = ?3)
         ORDER BY {order} LIMIT {WORD_LIMIT}",
        why_select()
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| ServerFnError::new(e.to_string()))?;
    let rows = stmt
        .query_map(rusqlite::params![book_id, level, want], |r| {
            let word: String = r.get(0)?;
            let head: Option<String> = r.get(1)?;
            Ok(ExcludedWord {
                head: head.filter(|h| *h != word),
                word,
                word_id: r.get(2)?,
                count: r.get(3)?,
                freq_pm: r.get(4)?,
                gloss: r.get(5)?,
                reason: r.get(6)?,
            })
        })
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .filter_map(Result::ok)
        .collect();
    Ok(rows)
}

/// Digit grouping: 14207 -> "14,207". These counts run to five figures and are
/// meant to be taken in at a glance.
pub fn commas(n: i64) -> String {
    let s = n.abs().to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3 + 1);
    if n < 0 {
        out.push('-');
    }
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The words page's "what's left out?" panel: the filter accounting for the current
/// book and merge level, plus the words themselves, rarest first.
#[component]
pub fn ExcludedPanel(
    book: Memo<i64>,
    level: Memo<i64>,
    open: RwSignal<bool>,
    /// Opens a word's detail sheet, so a left-out word can still be read and tagged.
    set_word: SignalSetter<Option<i64>>,
) -> impl IntoView {
    let reason = RwSignal::new(String::new());
    let sort = RwSignal::new("rare".to_string());
    // Both resources stay idle until the panel is opened — this is a whole-book
    // scan, and most visits to the words page never ask for it.
    let report = Resource::new(
        move || (book.get(), level.get(), open.get()),
        |(b, l, go)| async move {
            if go { exclusion_report(b, l).await.map(Some) } else { Ok(None) }
        },
    );
    let words = Resource::new(
        move || (book.get(), level.get(), reason.get(), sort.get(), open.get()),
        |(b, l, why, s, go)| async move {
            if go { excluded_words(b, l, why, s).await } else { Ok(Vec::new()) }
        },
    );
    // A book or level change invalidates the chosen bucket's counts, so start over
    // on "all" rather than leave a chip selected that may no longer exist.
    Effect::new(move |_| {
        let _ = (book.get(), level.get());
        reason.set(String::new());
    });

    view! {
        <Show when=move || open.get() fallback=|| ()>
            <section class="excl">
                <Suspense fallback=move || view! { <p class="loading">"counting…"</p> }>
                    {move || report.get().map(|res| match res {
                        Err(e) => view! { <p class="err">{err_text(&e)}</p> }.into_any(),
                        Ok(None) => ().into_any(),
                        Ok(Some(rep)) => {
                            let left_out: i64 = rep.reasons.iter().map(|r| r.count).sum();
                            let merged = rep.n_kept - rep.n_candidates;
                            view! {
                                <p class="excl-sum">
                                    <strong>{commas(left_out)}</strong>
                                    {format!(" of this book's {} different words never reach the list:",
                                             commas(rep.n_types))}
                                </p>
                                <div class="excl-chips">
                                    <button type="button" class="excl-chip"
                                        class:on=move || reason.get().is_empty()
                                        on:click=move |_| reason.set(String::new())>
                                        "all reasons"
                                    </button>
                                    {rep.reasons.iter().map(|r| {
                                        let (k, k2) = (r.key.clone(), r.key.clone());
                                        view! {
                                            <button type="button" class="excl-chip" title=r.note.clone()
                                                class:on=move || reason.get() == k2
                                                on:click=move |_| reason.set(k.clone())>
                                                {r.label.clone()}
                                                <span class="excl-n">{commas(r.count)}</span>
                                            </button>
                                        }
                                    }).collect_view()}
                                </div>
                                <p class="excl-kept">{if merged > 0 {
                                    format!("The other {} are scored, and merge into {} ranked entries.",
                                            commas(rep.n_kept), commas(rep.n_candidates))
                                } else {
                                    format!("The other {} are scored and ranked.", commas(rep.n_kept))
                                }}</p>
                            }.into_any()
                        }
                    })}
                </Suspense>

                <div class="excl-bar">
                    <select class="catsel" title="which left-out words to show first"
                        prop:value=move || sort.get()
                        on:change=move |ev| sort.set(event_target_value(&ev))>
                        <option value="rare">"rarest first"</option>
                        <option value="count">"most used in this book"</option>
                    </select>
                    <span class="excl-cap">{format!("showing the first {WORD_LIMIT}")}</span>
                </div>

                <Suspense fallback=move || view! { <p class="loading">"…"</p> }>
                    {move || words.get().map(|res| match res {
                        Err(e) => view! { <p class="err">{err_text(&e)}</p> }.into_any(),
                        Ok(list) if list.is_empty() =>
                            view! { <p class="excl-kept">"No words left out."</p> }.into_any(),
                        Ok(list) => view! {
                            <table class="excl-table">
                                <thead><tr>
                                    <th>"word"</th>
                                    <th class="num" title="times this form appears in this book">"in this book"</th>
                                    <th class="num" title="uses per million words of general English, the number the \"too common\" test reads">"uses per million"</th>
                                    <th>"left out because"</th>
                                </tr></thead>
                                <tbody>
                                    {list.into_iter().map(|w| {
                                        let wid = w.word_id;
                                        let gloss = w.gloss.clone().unwrap_or_default();
                                        view! {
                                            <tr>
                                                <td>
                                                    {match wid {
                                                        Some(id) => view! {
                                                            <span class="word"
                                                                on:click=move |_| set_word.set(Some(id))>
                                                                {w.word.clone()}</span>
                                                        }.into_any(),
                                                        None => view! {
                                                            <span class="excl-oov">{w.word.clone()}</span>
                                                        }.into_any(),
                                                    }}
                                                    {w.head.map(|h| view! {
                                                        <small class="excl-head"
                                                            title="the form this word merges into, which is what gets judged">
                                                            {format!(" → {h}")}</small>
                                                    })}
                                                    {(!gloss.is_empty()).then(|| view! {
                                                        <span class="excl-gloss">{gloss}</span>
                                                    })}
                                                </td>
                                                <td class="num">{w.count}</td>
                                                // A recorded 0 means the corpus has no
                                                // lowercase uses at all — same story as a
                                                // missing row, so it reads the same way.
                                                // Enough decimals that the rarest measured
                                                // words still differ from each other: the
                                                // corpus floor is a shade under 0.0004.
                                                <td class="num">{match w.freq_pm {
                                                    Some(f) if f >= 1.0 => format!("{f:.1}"),
                                                    Some(f) if f >= 0.001 => format!("{f:.3}"),
                                                    Some(f) if f >= 0.0001 => format!("{f:.4}"),
                                                    Some(f) if f > 0.0 => "<0.0001".to_string(),
                                                    _ => "—".to_string(),
                                                }}</td>
                                                <td class="excl-why">{reason_label(&w.reason)}</td>
                                            </tr>
                                        }
                                    }).collect_view()}
                                </tbody>
                            </table>
                        }.into_any(),
                    })}
                </Suspense>

                <p class="excl-foot">
                    {format!("A word is judged on the form it merges into, shown after the arrow. \
                              \"Too common\" means over {MAX_FREQ_PM_LC:.0} uses per million words of \
                              general English, and \"under {MIN_LEN} letters\" counts letters only. \
                              A dash means no measured frequency at all, which sorts last under \
                              \"rarest first\": those are mostly names, contractions and misreadings.")}
                </p>
            </section>
        </Show>
    }
}
