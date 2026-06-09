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
    pub etymology: Option<String>,
    pub category: Option<String>,
    pub cluster: Option<i64>,
    pub selected: bool,
    pub verdict: Option<String>,
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
        .query_map([], |r| {
            Ok(Book { id: r.get(0)?, title: r.get(1)?, n_selected: r.get(2)? })
        })
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| ServerFnError::new(e.to_string()))?);
    }
    Ok(out)
}

#[server]
pub async fn get_candidates(book_id: i64, limit: i32) -> Result<Vec<Candidate>, ServerFnError> {
    use rusqlite::Connection;
    let conn = Connection::open(db_path()).map_err(|e| ServerFnError::new(e.to_string()))?;
    let mut stmt = conn
        .prepare(
            "SELECT w.id, w.word, c.in_book, c.score, w.gloss, w.etymology_lang,
                    w.wordnet_category, c.cluster, c.selected, r.verdict
             FROM candidates c
             JOIN words w ON w.id = c.word_id
             LEFT JOIN ratings r ON r.book_id = c.book_id AND r.word_id = c.word_id
             WHERE c.book_id = ?1
             ORDER BY c.rank
             LIMIT ?2",
        )
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let rows = stmt
        .query_map((book_id, limit), |row| {
            Ok(Candidate {
                word_id: row.get(0)?,
                word: row.get(1)?,
                in_book: row.get(2)?,
                score: row.get(3)?,
                gloss: row.get(4)?,
                etymology: row.get(5)?,
                category: row.get(6)?,
                cluster: row.get(7)?,
                selected: row.get::<_, i64>(8)? != 0,
                verdict: row.get(9)?,
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

fn short(g: &Option<String>, n: usize) -> String {
    match g {
        None => String::new(),
        Some(s) => {
            let t: String = s.chars().take(n).collect();
            if s.chars().count() > n {
                format!("{t}…")
            } else {
                t
            }
        }
    }
}

#[component]
fn HomePage() -> impl IntoView {
    let book = RwSignal::new(1i64); // Moby-Dick by default
    let only_top = RwSignal::new(false);
    let rate = ServerAction::<SetRating>::new();
    let books = Resource::new(|| (), |_| list_books());
    let candidates = Resource::new(
        move || (book.get(), rate.version().get()),
        move |(b, _)| get_candidates(b, 400),
    );

    view! {
        <h1>"coolwords"</h1>
        <p class="sub">"Pick the interesting words. keep / reject / shadow — ratings persist."</p>

        <div class="bar">
            <Suspense fallback=move || view! { <span>"…"</span> }>
                {move || books.get().map(|res| match res {
                    Err(e) => view! { <span class="err">{format!("{e}")}</span> }.into_any(),
                    Ok(list) => view! {
                        <span class="label">"book: "</span>
                        {list.into_iter().map(|b| {
                            let id = b.id;
                            let active = move || book.get() == id;
                            view! {
                                <button
                                    class:active=active
                                    on:click=move |_| book.set(id)
                                >{format!("{} ({}★)", b.title, b.n_selected)}</button>
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

        <Suspense fallback=move || view! { <p class="loading">"Loading…"</p> }>
            {move || {
                candidates.get().map(|res| match res {
                    Err(e) => view! { <p class="err">{format!("Error: {e}")}</p> }.into_any(),
                    Ok(all) => {
                        let top = only_top.get();
                        let list: Vec<Candidate> =
                            all.into_iter().filter(|c| !top || c.selected).collect();
                        let total = list.len();
                        let kept = list.iter().filter(|c| c.verdict.as_deref() == Some("keep")).count();
                        view! {
                            <p class="counts">{format!("{total} shown · {kept} kept")}</p>
                            <table>
                                <thead>
                                    <tr>
                                        <th></th><th>"word"</th><th>"gloss"</th><th>"in bk"</th>
                                        <th>"score"</th><th>"origin"</th><th>"category"</th>
                                        <th>"cl"</th><th>"verdict"</th><th></th>
                                    </tr>
                                </thead>
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
                                        let gloss = short(&c.gloss, 95);
                                        let verdict_txt = c.verdict.clone().unwrap_or_default();
                                        let cluster_txt = c.cluster.map(|n| n.to_string()).unwrap_or_default();
                                        view! {
                                            <tr class=cls>
                                                <td class="star">{star}</td>
                                                <td class="word">{c.word.clone()}</td>
                                                <td class="gloss">{gloss}</td>
                                                <td class="num">{c.in_book}</td>
                                                <td class="num">{format!("{:.1}", c.score)}</td>
                                                <td>{c.etymology.clone().unwrap_or_default()}</td>
                                                <td>{c.category.clone().unwrap_or_default()}</td>
                                                <td class="num">{cluster_txt}</td>
                                                <td class="verdict">{verdict_txt}</td>
                                                <td class="actions">
                                                    {["keep", "reject", "shadow"].into_iter().map(|v| {
                                                        view! {
                                                            <ActionForm action=rate>
                                                                <input type="hidden" name="book_id" value=move || book.get().to_string()/>
                                                                <input type="hidden" name="word_id" value=wid.to_string()/>
                                                                <input type="hidden" name="verdict" value=v/>
                                                                <button type="submit" class=v>{v}</button>
                                                            </ActionForm>
                                                        }
                                                    }).collect_view()}
                                                </td>
                                            </tr>
                                        }
                                    }).collect_view()}
                                </tbody>
                            </table>
                        }.into_any()
                    }
                })
            }}
        </Suspense>
    }
}
