// Same reason as lib.rs: the deeply nested view! trees push the trait solver past
// its default depth (this bin monomorphizes the SSR render). Inner attribute → must
// precede all items in the crate root.
#![recursion_limit = "1024"]

#[cfg(feature = "ssr")]
#[tokio::main]
async fn main() {
    use axum::extract::DefaultBodyLimit;
    use axum::routing::get;
    use axum::Router;
    use leptos::logging::log;
    use leptos::prelude::*;
    use leptos_axum::{generate_route_list, LeptosRoutes};
    use coolwords_ui::app::*;
    use coolwords_ui::buildinfo;

    // Log one line with the version, build time and start time before anything else,
    // so the line appears even if startup fails. Home Assistant's add-on log has no
    // timestamps or version numbers, so this line shows which build is running.
    log!(
        "[coolwords] {} (built {}) starting at {}",
        buildinfo::VERSION,
        buildinfo::built_at(),
        buildinfo::started_at(),
    );

    let conf = get_configuration(None).unwrap();
    let mut leptos_options = conf.leptos_options;
    // Allow overriding the bind address at runtime (LEPTOS_SITE_ADDR also works, but
    // this is unambiguous for the tunnel / Linux / Home Assistant add-on deploys).
    if let Ok(bind) = std::env::var("COOLWORDS_BIND") {
        match bind.parse() {
            Ok(parsed) => leptos_options.site_addr = parsed,
            Err(e) => log!("ignoring invalid COOLWORDS_BIND={bind:?}: {e}"),
        }
    }
    let addr = leptos_options.site_addr;
    let routes = generate_route_list(App);

    let app = Router::new()
        // Returns the version, build time, start time and current time as plain text.
        // The route is outside the Leptos routes, so it responds even when a page fails
        // to render. It works through ingress and through the tunnel.
        .route("/version", get(|| async { buildinfo::report() }))
        .leptos_routes(&leptos_options, routes, {
            let leptos_options = leptos_options.clone();
            move || shell(leptos_options.clone())
        })
        .fallback(leptos_axum::file_and_error_handler(shell))
        // Raise axum's 2 MB default so drag-dropped EPUBs (the upload_book server
        // fn streams multipart bodies) aren't rejected. 64 MB headroom.
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
        .with_state(leptos_options);

    log!("listening on http://{}", &addr);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app.into_make_service()).await.unwrap();
}

#[cfg(not(feature = "ssr"))]
pub fn main() {
    // client builds use the hydrate() entry point in lib.rs
}
