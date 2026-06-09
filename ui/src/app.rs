use leptos::prelude::*;
use leptos_meta::{provide_meta_context, MetaTags, Stylesheet, Title};
use leptos_router::{
    components::{Route, Router, Routes},
    StaticSegment,
};
use serde::{Deserialize, Serialize};

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
    pub verdict: Option<String>,
    pub example: Option<String>,
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
    pub verdict: Option<String>,
    pub categories: Vec<String>,
    /// base lemma + its freq, only when the base is MORE common than this word
    /// (i.e. this looks like a rare variant of a common lemma).
    pub base: Option<(String, f64)>,
    /// other forms in THIS book sharing the stem (word, in-book count, freq/M)
    pub family: Vec<(String, i64, f64)>,
    pub relations: Vec<(String, String)>,
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
pub async fn get_candidates(
    book_id: i64,
    category: Option<String>,
    limit: i32,
) -> Result<Vec<Candidate>, ServerFnError> {
    use rusqlite::Connection;
    let conn = Connection::open(db_path()).map_err(|e| ServerFnError::new(e.to_string()))?;
    let mut stmt = conn
        .prepare(
            "SELECT w.id, w.word, c.in_book, c.score, w.gloss, w.etymology_lang, ln.name,
                    w.wordnet_category, c.cluster, c.selected, r.verdict, bo.example
             FROM candidates c
             JOIN words w ON w.id = c.word_id
             LEFT JOIN lang_names ln ON ln.code = w.etymology_lang
             LEFT JOIN book_occurrences bo ON bo.book_id = c.book_id AND bo.word_id = c.word_id
             LEFT JOIN ratings r ON r.book_id = c.book_id AND r.word_id = c.word_id
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
                verdict: row.get(10)?,
                example: row.get(11)?,
            })
        })
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| ServerFnError::new(e.to_string()))?);
    }
    Ok(out)
}

#[server]
pub async fn word_detail(book_id: i64, word_id: i64) -> Result<WordInfo, ServerFnError> {
    use rusqlite::{Connection, OptionalExtension};
    let conn = Connection::open(db_path()).map_err(|e| ServerFnError::new(e.to_string()))?;

    let (word, gloss, origin_code, origin_name, freq_pm, syllables, stem, in_book, example, verdict):
        (String, Option<String>, Option<String>, Option<String>, Option<f64>, Option<i64>,
         Option<String>, i64, Option<String>, Option<String>) = conn
        .query_row(
            "SELECT w.word, w.gloss, w.etymology_lang, ln.name, w.freq_pm, w.syllables, w.stem,
                    COALESCE(bo.count, 0), bo.example, r.verdict
             FROM words w
             LEFT JOIN lang_names ln ON ln.code = w.etymology_lang
             LEFT JOIN book_occurrences bo ON bo.word_id = w.id AND bo.book_id = ?1
             LEFT JOIN ratings r ON r.word_id = w.id AND r.book_id = ?1
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

    // base lemma, only surfaced when it is MORE common than this word
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

    // other forms in this book sharing the stem (include the base word itself)
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
        .prepare("SELECT rel, target FROM word_relation WHERE word_id = ?1 ORDER BY rel LIMIT 40")
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let relations: Vec<(String, String)> = relstmt
        .query_map([word_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .filter_map(Result::ok)
        .collect();

    Ok(WordInfo {
        word_id, word, gloss, origin_code, origin_name, freq_pm, syllables, in_book, example,
        verdict, categories, base, family, relations,
    })
}

#[server]
pub async fn set_rating(book_id: i64, word_id: i64, verdict: String) -> Result<(), ServerFnError> {
    use rusqlite::Connection;
    let conn = Connection::open(db_path()).map_err(|e| ServerFnError::new(e.to_string()))?;
    conn.execute(
        "INSERT INTO ratings(book_id, word_id, rater, verdict, ts)
         VALUES (?1, ?2, 'me', ?3, datetime('now'))
         ON CONFLICT(book_id, word_id, rater)
         DO UPDATE SET verdict = excluded.verdict, ts = excluded.ts",
        (book_id, word_id, verdict),
    )
    .map_err(|e| ServerFnError::new(e.to_string()))?;
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

#[component]
fn HomePage() -> impl IntoView {
    let book = RwSignal::new(1i64);
    let only_top = RwSignal::new(false);
    let category = RwSignal::new(None::<String>);
    let selected = RwSignal::new(None::<i64>);
    let rate = ServerAction::<SetRating>::new();

    let books = Resource::new(|| (), |_| list_books());
    let candidates = Resource::new(
        move || (book.get(), category.get(), rate.version().get()),
        move |(b, cat, _)| get_candidates(b, cat, 400),
    );
    let detail = Resource::new(
        move || (book.get(), selected.get(), rate.version().get()),
        move |(b, sel, _)| async move {
            match sel {
                Some(wid) => word_detail(b, wid).await.map(Some),
                None => Ok(None),
            }
        },
    );

    view! {
        <h1>"coolwords"</h1>
        <p class="sub">"Pick the interesting words. Click a word for detail, a category to filter."</p>

        <div class="bar">
            <Suspense fallback=move || view! { <span>"…"</span> }>
                {move || books.get().map(|res| match res {
                    Err(e) => view! { <span class="err">{format!("{e}")}</span> }.into_any(),
                    Ok(list) => view! {
                        <span class="label">"book: "</span>
                        {list.into_iter().map(|b| {
                            let id = b.id;
                            view! {
                                <button class:active=move || book.get() == id
                                    on:click=move |_| { book.set(id); selected.set(None); category.set(None); }>
                                    {format!("{} ({}★)", b.title, b.n_selected)}
                                </button>
                            }
                        }).collect_view()}
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
                <button class="clear" on:click=move |_| category.set(None)>"clear"</button>
            </p>
        </Show>

        <Suspense fallback=move || view! { <p class="loading">"Loading…"</p> }>
            {move || candidates.get().map(|res| match res {
                Err(e) => view! { <p class="err">{format!("Error: {e}")}</p> }.into_any(),
                Ok(all) => {
                    let top = only_top.get();
                    let list: Vec<Candidate> = all.into_iter().filter(|c| !top || c.selected).collect();
                    let total = list.len();
                    let kept = list.iter().filter(|c| c.verdict.as_deref() == Some("keep")).count();
                    view! {
                        <p class="counts">{format!("{total} shown · {kept} kept")}</p>
                        <table>
                            <thead><tr>
                                <th></th><th>"word"</th><th>"gloss"</th><th>"in bk"</th><th>"score"</th>
                                <th>"origin"</th><th>"category"</th><th>"cl"</th><th>"verdict"</th><th></th>
                            </tr></thead>
                            <tbody>
                                {list.into_iter().map(|c| {
                                    let wid = c.word_id;
                                    let cls = match c.verdict.as_deref() {
                                        Some("keep") => "row keep",
                                        Some("reject") => "row reject",
                                        Some("shadow") => "row shadow",
                                        _ => "row",
                                    };
                                    let star = if c.selected { "★" } else { "" };
                                    let gloss = short(&c.gloss, 90);
                                    let example = c.example.clone().unwrap_or_default();
                                    let origin_disp = c.origin_name.clone().or_else(|| c.origin_code.clone()).unwrap_or_default();
                                    let origin_title = c.origin_code.clone().unwrap_or_default();
                                    let cat = c.category.clone();
                                    let cat_click = cat.clone();
                                    let verdict_txt = c.verdict.clone().unwrap_or_default();
                                    let cluster_txt = c.cluster.map(|n| n.to_string()).unwrap_or_default();
                                    view! {
                                        <tr class=cls>
                                            <td class="star">{star}</td>
                                            <td class="word" title=example on:click=move |_| selected.set(Some(wid))>
                                                {c.word.clone()}
                                            </td>
                                            <td class="gloss">{gloss}</td>
                                            <td class="num">{c.in_book}</td>
                                            <td class="num">{format!("{:.1}", c.score)}</td>
                                            <td title=origin_title>{origin_disp}</td>
                                            <td class="cat"
                                                on:click=move |_| { if let Some(cc) = cat_click.clone() { category.set(Some(cc)); } }>
                                                {cat.clone().unwrap_or_default()}
                                            </td>
                                            <td class="num">{cluster_txt}</td>
                                            <td class="verdict">{verdict_txt}</td>
                                            <td class="actions">
                                                {["keep", "reject", "shadow"].into_iter().map(|v| view! {
                                                    <ActionForm action=rate>
                                                        <input type="hidden" name="book_id" value=move || book.get().to_string()/>
                                                        <input type="hidden" name="word_id" value=wid.to_string()/>
                                                        <input type="hidden" name="verdict" value=v/>
                                                        <button type="submit" class=v>{v}</button>
                                                    </ActionForm>
                                                }).collect_view()}
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
                <button class="close" on:click=move |_| selected.set(None)>"×"</button>
                <Suspense fallback=move || view! { <p class="loading">"…"</p> }>
                    {move || detail.get().map(|res| match res {
                        Err(e) => view! { <p class="err">{format!("{e}")}</p> }.into_any(),
                        Ok(None) => ().into_any(),
                        Ok(Some(d)) => {
                            let origin = d.origin_name.clone().or_else(|| d.origin_code.clone()).unwrap_or_default();
                            let wid = d.word_id;
                            view! {
                                <h2>{d.word.clone()}</h2>
                                <p class="gloss">{d.gloss.clone().unwrap_or_default()}</p>
                                {d.example.clone().map(|ex| view! { <blockquote class="ex">{ex}</blockquote> })}
                                <ul class="meta">
                                    <li>{format!("in this book: {}×", d.in_book)}</li>
                                    <li>{format!("frequency: {:.3}/M", d.freq_pm.unwrap_or(0.0))}</li>
                                    <li>{format!("syllables: {}", d.syllables.map(|n| n.to_string()).unwrap_or_default())}</li>
                                    <li>{format!("origin: {origin}")}</li>
                                </ul>
                                <Show when={let c = d.categories.clone(); move || !c.is_empty()} fallback=|| ()>
                                    <p class="caps">"categories: "
                                        {d.categories.clone().into_iter().map(|cat| {
                                            let cc = cat.clone();
                                            view! {
                                                <button class="catchip" on:click=move |_| category.set(Some(cc.clone()))>{cat}</button>
                                            }
                                        }).collect_view()}
                                    </p>
                                </Show>
                                <Show when={let b = d.base.clone(); move || b.is_some()} fallback=|| ()>
                                    <p class="base">
                                        {let b = d.base.clone().unwrap();
                                         format!("likely a variant of a more common word: {} ({:.1}/M)", b.0, b.1)}
                                    </p>
                                </Show>
                                <Show when={let f = d.family.clone(); move || f.len() > 1} fallback=|| ()>
                                    <p class="caps">"related forms in this book:"</p>
                                    <ul class="family">
                                        {d.family.clone().into_iter().map(|(w, n, fp)| view! {
                                            <li>{format!("{w} — {n}× here, {fp:.1}/M overall")}</li>
                                        }).collect_view()}
                                    </ul>
                                </Show>
                                <Show when={let r = d.relations.clone(); move || !r.is_empty()} fallback=|| ()>
                                    <p class="caps">"WordNet relations:"</p>
                                    <ul class="rels">
                                        {d.relations.clone().into_iter().map(|(rel, tgt)| view! {
                                            <li><span class="rel">{rel}</span>" "{tgt}</li>
                                        }).collect_view()}
                                    </ul>
                                </Show>
                                <div class="actions detail-actions">
                                    {["keep", "reject", "shadow"].into_iter().map(|v| view! {
                                        <ActionForm action=rate>
                                            <input type="hidden" name="book_id" value=move || book.get().to_string()/>
                                            <input type="hidden" name="word_id" value=wid.to_string()/>
                                            <input type="hidden" name="verdict" value=v/>
                                            <button type="submit" class=v>{v}</button>
                                        </ActionForm>
                                    }).collect_view()}
                                </div>
                            }.into_any()
                        }
                    })}
                </Suspense>
            </aside>
        </Show>
    }
}
