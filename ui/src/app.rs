use std::collections::{HashMap, HashSet};

use leptos::prelude::*;
use leptos_meta::{provide_meta_context, MetaTags, Stylesheet, Title};
use leptos_router::components::{Route, Router, Routes};
use leptos_router::hooks::query_signal;
use leptos_router::StaticSegment;
use serde::{Deserialize, Serialize};

pub const TAGS: &[&str] = &[
    "useful", "strange", "interesting", "aesthetic", "emblematic", "category-pick",
];

#[derive(Clone, Copy)]
pub struct TagStore(pub RwSignal<HashMap<(i64, i64), HashSet<String>>>);

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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CatCount {
    pub category: String,
    pub count: i64,
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
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RelTarget {
    pub rel: String,
    pub target: String,
    pub target_word_id: Option<i64>,
    pub in_book: bool,
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
    pub base: Option<(String, f64)>,
    pub family: Vec<(String, i64, f64)>,
    pub relations: Vec<RelTarget>,
    pub trajectory: Vec<(i32, f64)>,
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

#[server]
pub async fn list_books() -> Result<Vec<Book>, ServerFnError> {
    use rusqlite::Connection;
    let conn = Connection::open(db_path()).map_err(|e| ServerFnError::new(e.to_string()))?;
    let mut stmt = conn
        .prepare(
            "SELECT b.id, COALESCE(b.title, b.slug),
                    (SELECT count(*) FROM candidates c WHERE c.book_id = b.id AND c.selected = 1)
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
pub async fn list_categories(book_id: i64) -> Result<Vec<CatCount>, ServerFnError> {
    use rusqlite::Connection;
    let conn = Connection::open(db_path()).map_err(|e| ServerFnError::new(e.to_string()))?;
    let mut stmt = conn
        .prepare(
            "SELECT wc.category, count(DISTINCT c.word_id) n
             FROM candidates c JOIN word_category wc ON wc.word_id = c.word_id
             WHERE c.book_id = ?1 GROUP BY wc.category ORDER BY n DESC, wc.category",
        )
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let rows = stmt
        .query_map([book_id], |r| Ok(CatCount { category: r.get(0)?, count: r.get(1)? }))
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| ServerFnError::new(e.to_string()))?);
    }
    Ok(out)
}

#[cfg(feature = "ssr")]
fn load_tags(conn: &rusqlite::Connection, book_id: i64) -> Result<HashMap<i64, Vec<String>>, ServerFnError> {
    let mut stmt = conn
        .prepare("SELECT word_id, tag FROM word_tags WHERE book_id = ?1 AND rater = 'me'")
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let mut map: HashMap<i64, Vec<String>> = HashMap::new();
    let rows = stmt
        .query_map([book_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    for r in rows {
        let (wid, tag) = r.map_err(|e| ServerFnError::new(e.to_string()))?;
        map.entry(wid).or_default().push(tag);
    }
    Ok(map)
}

#[server]
pub async fn get_candidates(
    book_id: i64,
    category: Option<String>,
    limit: i32,
) -> Result<Vec<Candidate>, ServerFnError> {
    use rusqlite::Connection;
    let conn = Connection::open(db_path()).map_err(|e| ServerFnError::new(e.to_string()))?;
    let tags = load_tags(&conn, book_id)?;
    let mut stmt = conn
        .prepare(
            "SELECT w.id, w.word, c.in_book, c.score, w.gloss, w.etymology_lang, ln.name,
                    w.wordnet_category, c.cluster, c.selected, bo.example
             FROM candidates c
             JOIN words w ON w.id = c.word_id
             LEFT JOIN lang_names ln ON ln.code = w.etymology_lang
             LEFT JOIN book_occurrences bo ON bo.book_id = c.book_id AND bo.word_id = c.word_id
             WHERE c.book_id = ?1
               AND (?2 IS NULL OR EXISTS (
                     SELECT 1 FROM word_category wc WHERE wc.word_id = w.id AND wc.category = ?2))
             ORDER BY c.rank
             LIMIT ?3",
        )
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let rows = stmt
        .query_map(rusqlite::params![book_id, category, limit], |row| {
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
            })
        })
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let mut out = Vec::new();
    for r in rows {
        let mut c = r.map_err(|e| ServerFnError::new(e.to_string()))?;
        if let Some(t) = tags.get(&c.word_id) {
            c.tags = t.clone();
        }
        out.push(c);
    }
    Ok(out)
}

#[server]
pub async fn word_detail(book_id: i64, word_id: i64) -> Result<WordInfo, ServerFnError> {
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

    let mut family = Vec::new();
    if let Some(st) = &stem {
        let mut fstmt = conn
            .prepare(
                "SELECT w.word, bo.count, w.freq_pm
                 FROM book_occurrences bo JOIN words w ON w.id = bo.word_id
                 WHERE bo.book_id = ?1 AND w.alpha_only = 1 AND (w.stem = ?2 OR w.word = ?2)
                 ORDER BY w.freq_pm DESC LIMIT 15",
            )
            .map_err(|e| ServerFnError::new(e.to_string()))?;
        let rows = fstmt
            .query_map(rusqlite::params![book_id, st], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, Option<f64>>(2)?.unwrap_or(0.0)))
            })
            .map_err(|e| ServerFnError::new(e.to_string()))?;
        for r in rows {
            family.push(r.map_err(|e| ServerFnError::new(e.to_string()))?);
        }
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

    Ok(WordInfo {
        word_id, word, gloss, origin_code, origin_name, freq_pm, syllables, in_book, example,
        book_year, categories, base, family, relations, trajectory,
    })
}

#[server]
pub async fn set_tag(book_id: i64, word_id: i64, tag: String, on: bool) -> Result<(), ServerFnError> {
    use rusqlite::Connection;
    if !TAGS.contains(&tag.as_str()) {
        return Err(ServerFnError::new("unknown tag"));
    }
    let conn = Connection::open(db_path()).map_err(|e| ServerFnError::new(e.to_string()))?;
    if on {
        conn.execute(
            "INSERT OR IGNORE INTO word_tags(book_id, word_id, tag, rater, ts)
             VALUES (?1, ?2, ?3, 'me', datetime('now'))",
            (book_id, word_id, &tag),
        )
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    } else {
        conn.execute(
            "DELETE FROM word_tags WHERE book_id = ?1 AND word_id = ?2 AND tag = ?3 AND rater = 'me'",
            (book_id, word_id, &tag),
        )
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    }
    Ok(())
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

#[component]
fn TagChips(book_id: i64, word_id: i64) -> impl IntoView {
    let store = expect_context::<TagStore>();
    let set_tag = ServerAction::<SetTag>::new();
    let key = (book_id, word_id);
    view! {
        <span class="chips">
            {TAGS.iter().map(|&tag| {
                let on_tag = tag.to_string();
                let is_on = move || store.0.with(|m| m.get(&key).is_some_and(|s| s.contains(on_tag.as_str())));
                let click_tag = tag.to_string();
                let on_click = move |_| {
                    let now = store.0.with(|m| m.get(&key).is_some_and(|s| s.contains(click_tag.as_str())));
                    let next = !now;
                    store.0.update(|m| {
                        let set = m.entry(key).or_default();
                        if next { set.insert(click_tag.clone()); } else { set.remove(click_tag.as_str()); }
                    });
                    set_tag.dispatch(SetTag { book_id, word_id, tag: click_tag.clone(), on: next });
                };
                view! {
                    <button type="button" class="chip" class:on=is_on on:click=on_click>{tag}</button>
                }
            }).collect_view()}
        </span>
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
        let frac = ((bdec - min_dec as f64) / span).clamp(0.0, 1.0);
        (frac * (w - (w / n)) + (w / n) / 2.0, y)
    });
    view! {
        <svg class="traj" width=w height=h viewBox=format!("0 0 {w} {h}") role="img" aria-label="usage over time">
            {bars.into_iter().map(|(x, y, bh)| view! {
                <rect x=x y=y width=bar_w height=bh class="traj-bar"/>
            }).collect_view()}
            {marker.map(|(mx, yr)| view! {
                <line x1=mx y1=0.0 x2=mx y2=plot_h class="traj-marker"/>
                <text x=mx y=h text-anchor="middle" class="traj-yr">{yr.to_string()}</text>
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
    let (selected, set_word) = query_signal::<i64>("word");
    let book = Memo::new(move |_| book_q.get().unwrap_or(1));
    let only_top = RwSignal::new(false);

    let tag_store = TagStore(RwSignal::new(HashMap::new()));
    provide_context(tag_store);

    let books = Resource::new(|| (), |_| list_books());
    let categories = Resource::new(move || book.get(), list_categories);
    let candidates = Resource::new(
        move || (book.get(), category.get()),
        move |(b, cat)| get_candidates(b, cat, 400),
    );
    let detail = Resource::new(
        move || (book.get(), selected.get()),
        move |(b, sel)| async move {
            match sel {
                Some(wid) => word_detail(b, wid).await.map(Some),
                None => Ok(None),
            }
        },
    );

    // seed the tag store from loaded candidates (client-side, after render)
    Effect::new(move |_| {
        if let Some(Ok(list)) = candidates.get() {
            let b = book.get();
            tag_store.0.update(|m| {
                for c in &list {
                    m.entry((b, c.word_id)).or_default().extend(c.tags.iter().cloned());
                }
            });
        }
    });

    view! {
        <h1>"coolwords"</h1>
        <p class="sub">"Tag the interesting words. Click a word for detail; click a category to filter."</p>

        <div class="bar">
            <Suspense fallback=move || view! { <span>"…"</span> }>
                {move || books.get().map(|res| match res {
                    Err(e) => view! { <span class="err">{format!("{e}")}</span> }.into_any(),
                    Ok(list) => view! {
                        {list.into_iter().map(|b| {
                            let id = b.id;
                            view! {
                                <button class:active=move || book.get() == id
                                    on:click=move |_| { set_book.set(Some(id)); set_word.set(None); set_cat.set(None); }>
                                    {format!("{} ({}★)", b.title, b.n_selected)}
                                </button>
                            }
                        }).collect_view()}
                    }.into_any(),
                })}
            </Suspense>
            <Suspense fallback=|| ()>
                {move || categories.get().map(|res| match res {
                    Err(_) => ().into_any(),
                    Ok(cats) => view! {
                        <select class="catsel"
                            prop:value=move || category.get().unwrap_or_default()
                            on:change=move |ev| {
                                let v = event_target_value(&ev);
                                if v.is_empty() { set_cat.set(None); } else { set_cat.set(Some(v)); }
                            }>
                            <option value="">"all categories"</option>
                            {cats.into_iter().map(|cc| view! {
                                <option value=cc.category.clone()>{format!("{} ({})", cc.category, cc.count)}</option>
                            }).collect_view()}
                        </select>
                    }.into_any(),
                })}
            </Suspense>
            <label class="toggle">
                <input type="checkbox" prop:checked=move || only_top.get()
                    on:change=move |_| only_top.update(|v| *v = !*v)/>
                " varied top-20 only"
            </label>
        </div>

        <Show when=move || category.get().is_some() fallback=|| ()>
            <p class="filter">
                {move || format!("category: {}", category.get().unwrap_or_default())}
                <button class="clear" on:click=move |_| set_cat.set(None)>"clear"</button>
            </p>
        </Show>

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
                                    let star = if c.selected { "★" } else { "" };
                                    let gloss = short(&c.gloss, 90);
                                    let example = c.example.clone().unwrap_or_default();
                                    let origin_disp = c.origin_name.clone().or_else(|| c.origin_code.clone()).unwrap_or_default();
                                    let origin_title = c.origin_code.clone().unwrap_or_default();
                                    let cat = c.category.clone();
                                    let cat_click = cat.clone();
                                    let cluster_txt = c.cluster.map(|n| n.to_string()).unwrap_or_default();
                                    view! {
                                        <tr class="row" class:tagged=move || store_has_tag(b, wid)>
                                            <td class="star">{star}</td>
                                            <td class="word" title=example on:click=move |_| set_word.set(Some(wid))>
                                                {c.word.clone()}
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
                                            <td class="actions"><TagChips book_id=b word_id=wid/></td>
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
                            // group relations by rel (already ordered)
                            let mut groups: Vec<(String, Vec<RelTarget>)> = Vec::new();
                            for rt in d.relations.clone() {
                                if let Some(last) = groups.last_mut() {
                                    if last.0 == rt.rel { last.1.push(rt); continue; }
                                }
                                groups.push((rt.rel.clone(), vec![rt]));
                            }
                            view! {
                                <h2>{d.word.clone()}</h2>
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
                                <Show when={let f = d.family.clone(); move || f.len() > 1} fallback=|| ()>
                                    <p class="caps">"related forms in this book:"</p>
                                    <ul class="family">
                                        {d.family.clone().into_iter().map(|(fw, n, fp)| view! {
                                            <li>{format!("{fw} — {n}× here, {fp:.1}/M overall")}</li>
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
                                <div class="actions detail-actions"><TagChips book_id=b word_id=wid/></div>
                            }.into_any()
                        }
                    })}
                </Suspense>
            </aside>
        </Show>
    }
}

fn store_has_tag(book_id: i64, word_id: i64) -> bool {
    expect_context::<TagStore>().0.with(|m| m.get(&(book_id, word_id)).is_some_and(|s| !s.is_empty()))
}
