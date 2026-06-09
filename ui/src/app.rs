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
pub struct Candidate {
    pub word_id: i64,
    pub word: String,
    pub in_book: i64,
    pub score: f64,
    pub etymology: Option<String>,
    pub category: Option<String>,
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
pub async fn get_candidates(book_id: i64, limit: i32) -> Result<Vec<Candidate>, ServerFnError> {
    use rusqlite::Connection;
    let conn = Connection::open(db_path()).map_err(|e| ServerFnError::new(e.to_string()))?;
    let mut stmt = conn
        .prepare(
            "SELECT w.id, w.word, c.in_book, c.score, w.etymology_lang, w.wordnet_category, r.verdict
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
                etymology: row.get(4)?,
                category: row.get(5)?,
                verdict: row.get(6)?,
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

#[component]
fn HomePage() -> impl IntoView {
    // Alice in Wonderland (book_id 2) — the smaller, cleaner list, to start.
    let book_id = 2i64;
    let rate = ServerAction::<SetRating>::new();
    let candidates = Resource::new(
        move || rate.version().get(),
        move |_| get_candidates(book_id, 300),
    );

    view! {
        <h1>"coolwords"</h1>
        <p class="sub">"Rate the interesting-word candidates: keep / reject / shadow."</p>
        <Suspense fallback=move || view! { <p class="loading">"Loading…"</p> }>
            {move || {
                candidates.get().map(|res| match res {
                    Err(e) => view! { <p class="err">{format!("Error: {e}")}</p> }.into_any(),
                    Ok(list) => {
                        let total = list.len();
                        let kept = list.iter().filter(|c| c.verdict.as_deref() == Some("keep")).count();
                        view! {
                            <p class="counts">{format!("{total} candidates · {kept} kept")}</p>
                            <table>
                                <thead>
                                    <tr>
                                        <th>"word"</th><th>"in book"</th><th>"score"</th>
                                        <th>"origin"</th><th>"category"</th><th>"verdict"</th><th></th>
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
                                        let verdict_txt = c.verdict.clone().unwrap_or_default();
                                        view! {
                                            <tr class=cls>
                                                <td class="word">{c.word.clone()}</td>
                                                <td class="num">{c.in_book}</td>
                                                <td class="num">{format!("{:.1}", c.score)}</td>
                                                <td>{c.etymology.clone().unwrap_or_default()}</td>
                                                <td>{c.category.clone().unwrap_or_default()}</td>
                                                <td class="verdict">{verdict_txt}</td>
                                                <td class="actions">
                                                    {["keep", "reject", "shadow"].into_iter().map(|v| {
                                                        view! {
                                                            <ActionForm action=rate>
                                                                <input type="hidden" name="book_id" value=book_id.to_string()/>
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
