//! The words page's "excluded words" panel: which of a book's distinct words are
//! excluded from the ranking, and the reason for each one.
//!
//! `ingest/score.py` excludes most of a book's distinct words before it ranks the
//! rest: forms of other words, proper nouns, words that are too common, and words
//! that are not in the dictionary. The words page lists only the ranked words, so
//! this panel is the only place that shows how many words were excluded and which
//! words they are.
//!
//! `why_select` repeats the scorer's tests as one SQL `CASE`, which gives each
//! distinct word in a book exactly one result: ranked, or the key of one reason from
//! [`reasons`]. So the number of ranked words (`n_kept`) plus every reason's count
//! always equals `books.n_types`. If `score.py` gains a test that `why_select`
//! lacks, the words that the new test excludes pass every test in the `CASE` and
//! are counted as ranked, and `n_kept` becomes larger than the sum of
//! `candidates.n_forms` for that book and merge level.
//!
//! # Matching the scorer
//!
//! Each reason key matches one test in `ingest/score.py`:
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
//! `score_group` checks `not alpha`, `not inwikt`, `formof`, `proper` and
//! `offensive` in a single `or`, so the scorer gives those tests no order. The
//! order of `nowikt`, `formof`, `proper` and `offensive` in the `CASE` is this
//! module's choice: a word that fails more than one of them gets the reason that
//! comes first in the table.
//!
//! `nodict` and `nonalpha` test the word as it appears in the book. Every later test
//! reads the word that the book's word is merged into at the current merge level,
//! as the scorer does.
//! At merge level 2, "whales" is excluded because "whale" is too common, and the
//! panel's table shows the row as `whales → whale`.

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
/// Maximum number of words [`excluded_words`] returns. One reason can exclude
/// thousands of words from a book, and a table that long makes the page slow.
const WORD_LIMIT: i32 = 200;

/// The reasons a word is excluded, in the order `why_select` tests them. Each entry
/// is (key, chip label, chip tooltip). The UI sends `key` back to
/// [`excluded_words`] to list the words for one reason.
pub fn reasons() -> [(&'static str, String, String); 9] {
    let r = |key, label: &str, note: &str| (key, label.to_string(), note.to_string());
    [
        r("nodict", "not a dictionary word", "a name, a typo, a scanning error, or simply missing from the dictionary"),
        r("nonalpha", "characters other than a–z", "contains a hyphen, apostrophe, digit or accented letter, like \"man-of-war\", \"don't\" or \"café\""),
        r("nowikt", "no Wiktionary entry", "Wiktionary has no entry for this word, so it can't be checked for names, forms of other words or offensive meanings"),
        r("formof", "form of another word", "a form of another word in Wiktionary, such as a plural, past tense or alternative spelling. Forms are left out unless the merge setting combines them with their base word."),
        r("proper", "proper noun", "names of people, places and brands"),
        r("offensive", "offensive", "at least one meaning is labelled in Wiktionary as offensive, vulgar, derogatory, pejorative or a slur"),
        r("capitalised", "usually capitalised", "more than half of its uses in general English are capitalised, so it is treated as a name"),
        r("short", &format!("under {MIN_LEN} letters"), &format!("words shorter than {MIN_LEN} letters are not ranked")),
        r("common", "too common", &format!("used more than {MAX_FREQ_PM_LC:.0} times per million words of general English")),
    ]
}

pub fn reason_label(key: &str) -> String {
    reasons()
        .into_iter()
        .find(|(k, _, _)| *k == key)
        .map(|(_, label, _)| label)
        .unwrap_or_else(|| "excluded".to_string())
}

/// One reason, with the number of distinct words it excludes from one book at one
/// merge level.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExcludeReason {
    pub key: String,
    pub label: String,
    pub note: String,
    pub count: i64,
}

/// How many of a book's distinct words are ranked, and how many each reason
/// excludes, at one merge level. `n_kept` plus every reason's `count` equals
/// `n_types`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExcludeReport {
    /// Distinct words in the book (`books.n_types`).
    pub n_types: i64,
    /// Distinct words that pass every test and are ranked, on their own or merged
    /// with other forms.
    pub n_kept: i64,
    /// Entries in the ranked list at this merge level (`candidates` rows). Merged
    /// forms are one entry, so this is smaller than `n_kept` when any forms are merged.
    pub n_candidates: i64,
    /// Every reason that excludes at least one word, with the most words first.
    pub reasons: Vec<ExcludeReason>,
}

/// One excluded word.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExcludedWord {
    /// The word as it appears in the book.
    pub word: String,
    /// The word that `word` is merged into, when it differs from `word`. The reason
    /// applies to this word.
    pub head: Option<String>,
    /// Id of the word the reason applies to (`head`, or `word` when `head` is
    /// `None`), for opening its detail sheet. `None` when `word` is not in the
    /// dictionary.
    pub word_id: Option<i64>,
    /// Times this spelling appears in the book.
    pub count: i64,
    /// Lowercase uses per million words of general English, with merged forms added
    /// together. The "too common" test compares this number with `MAX_FREQ_PM_LC`.
    /// `None` when there is no frequency data.
    pub freq_pm: Option<f64>,
    pub gloss: Option<String>,
    pub reason: String,
}

/// One row per distinct word in the book, with the columns the panel's table shows
/// and `why`: the key of the reason that excludes the word, or `''` when the word is
/// ranked. `?1` is the book id and `?2` the merge level. The module docs list the
/// `CASE` tests in order. A word whose merged group is in `candidates` is ranked,
/// whatever the later tests say.
///
/// `tw` is the `words` row of the word as it appears in the book, `rw` is the
/// `words` row of the word that the book's word is merged into (the same row as
/// `tw` when nothing is merged), and `lf` (`lemma_freq`) has the combined frequency
/// of every form merged into `rw`. `score.py` reads the same three rows.
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

    let mut found: Vec<ExcludeReason> = reasons()
        .into_iter()
        .filter_map(|(key, label, note)| {
            let count = *counts.get(key)?;
            (count > 0).then(|| ExcludeReason { key: key.to_string(), label, note, count })
        })
        .collect();
    found.sort_by(|a, b| b.count.cmp(&a.count));

    Ok(ExcludeReport {
        n_types,
        n_kept: counts.get("").copied().unwrap_or(0),
        n_candidates,
        reasons: found,
    })
}

/// The excluded words, at most `WORD_LIMIT` of them. An empty `reason` lists the
/// words for every reason. `sort` is "rare" (default) for the lowest general-English
/// frequency first, or "count" for the most uses in this book first.
#[server]
pub async fn excluded_words(
    book_id: i64,
    level: i64,
    reason: String,
    sort: String,
) -> Result<Vec<ExcludedWord>, ServerFnError> {
    let conn = rusqlite::Connection::open(crate::app::db_path())
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    // "rare" lists the lowest general-English frequency first, with words that have
    // no frequency data last. "count" lists the most uses in this book first. Each
    // order breaks ties with the other order.
    let order = match sort.as_str() {
        "count" => format!("n DESC, {UNMEASURED_LAST} ASC, pm ASC"),
        _ => format!("{UNMEASURED_LAST} ASC, pm ASC, n DESC"),
    };
    // A `reason` that is not a reason key lists the words for every reason.
    let want = if reasons().iter().any(|(k, _, _)| *k == reason) { reason } else { String::new() };
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

/// The words page's "excluded words" panel: for the current book and merge level,
/// the number of distinct words each reason excludes, and a table of the excluded
/// words.
#[component]
pub fn ExcludedPanel(
    book: Memo<i64>,
    level: Memo<i64>,
    open: RwSignal<bool>,
    /// Opens a word's detail sheet, so an excluded word can still be read and tagged.
    set_word: SignalSetter<Option<i64>>,
) -> impl IntoView {
    // The reason chip the user clicked, with the book and merge level shown at the
    // time. `reason` is that chip's reason while the same book and merge level are
    // shown, and "" (all reasons) otherwise, because another book or merge level may
    // have no words for that reason.
    let chosen = RwSignal::new((0_i64, 0_i64, String::new()));
    let reason = Memo::new(move |_| {
        chosen.with(|(b, l, key)| {
            if (*b, *l) == (book.get(), level.get()) { key.clone() } else { String::new() }
        })
    });
    let choose = move |key: String| chosen.set((book.get_untracked(), level.get_untracked(), key));
    let sort = RwSignal::new("rare".to_string());
    // Both resources fetch nothing until the panel is open. Each query scans every
    // distinct word in the book, and most visits to the words page never open the
    // panel.
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
                                    {format!(" of this book's {} distinct words are excluded from the ranking:",
                                             commas(rep.n_types))}
                                </p>
                                <div class="excl-chips">
                                    <button type="button" class="excl-chip"
                                        class:on=move || reason.get().is_empty()
                                        on:click=move |_| choose(String::new())>
                                        "all reasons"
                                    </button>
                                    {rep.reasons.iter().map(|r| {
                                        let (k, k2) = (r.key.clone(), r.key.clone());
                                        view! {
                                            <button type="button" class="excl-chip" title=r.note.clone()
                                                class:on=move || reason.get() == k2
                                                on:click=move |_| choose(k.clone())>
                                                {r.label.clone()}
                                                <span class="excl-n">{commas(r.count)}</span>
                                            </button>
                                        }
                                    }).collect_view()}
                                </div>
                                <p class="excl-kept">{if merged > 0 {
                                    format!("The other {} distinct words are ranked. Merged forms count as one, \
                                             so there are {} ranked words.",
                                            commas(rep.n_kept), commas(rep.n_candidates))
                                } else {
                                    format!("The other {} distinct words are ranked.", commas(rep.n_kept))
                                }}</p>
                            }.into_any()
                        }
                    })}
                </Suspense>

                <div class="excl-bar">
                    <select class="catsel" title="order of the excluded words"
                        prop:value=move || sort.get()
                        on:change=move |ev| sort.set(event_target_value(&ev))>
                        <option value="rare">"rarest first"</option>
                        <option value="count">"most used in this book"</option>
                    </select>
                    // Shown only when the selected reason has more words than the
                    // list returns.
                    <Suspense fallback=|| ()>
                        {move || {
                            let rep = report.get()?.ok().flatten()?;
                            let why = reason.get();
                            let total: i64 = rep.reasons.iter()
                                .filter(|r| why.is_empty() || r.key == why)
                                .map(|r| r.count)
                                .sum();
                            (total > i64::from(WORD_LIMIT)).then(|| view! {
                                <span class="excl-cap">
                                    {format!("first {WORD_LIMIT} of {} words shown", commas(total))}
                                </span>
                            })
                        }}
                    </Suspense>
                </div>

                <Suspense fallback=move || view! { <p class="loading">"…"</p> }>
                    {move || words.get().map(|res| match res {
                        Err(e) => view! { <p class="err">{err_text(&e)}</p> }.into_any(),
                        Ok(list) if list.is_empty() =>
                            view! { <p class="excl-kept">"No excluded words."</p> }.into_any(),
                        Ok(list) => view! {
                            <table class="excl-table">
                                <thead><tr>
                                    <th>"word"</th>
                                    <th class="num" title="times this spelling appears in this book">"uses in this book"</th>
                                    <th class="num" title="lowercase uses per million words of general English, with merged forms added together">
                                        "uses per million (general English)"</th>
                                    <th>"reason"</th>
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
                                                            title="the word this form is merged into; the reason applies to that word">
                                                            {format!(" → {h}")}</small>
                                                    })}
                                                    {(!gloss.is_empty()).then(|| view! {
                                                        <span class="excl-gloss">{gloss}</span>
                                                    })}
                                                </td>
                                                <td class="num">{w.count}</td>
                                                // A frequency of 0 means the corpus has
                                                // no lowercase uses of the word, so the
                                                // cell shows "—", the same as for no
                                                // frequency data. Small values get 3 or 4
                                                // decimal places because the lowest
                                                // frequency in the corpus is just under
                                                // 0.0004 per million.
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
                    "→ shows the word a form is merged into. The reason applies to that word."
                </p>
                <p class="excl-foot">
                    "— under \"uses per million\" means no frequency data. These words are mostly \
                     names, contractions and misreadings, and are listed last under \"rarest first\"."
                </p>
                <p class="excl-foot">
                    {format!("\"Too common\" means more than {MAX_FREQ_PM_LC:.0} lowercase uses per \
                              million words of general English, with merged forms added together.")}
                </p>
            </section>
        </Show>
    }
}
