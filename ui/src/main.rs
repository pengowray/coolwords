#[cfg(feature = "ssr")]
#[tokio::main]
async fn main() {
    use axum::extract::DefaultBodyLimit;
    use axum::Router;
    use leptos::logging::log;
    use leptos::prelude::*;
    use leptos_axum::{generate_route_list, LeptosRoutes};
    use coolwords_ui::app::*;

    let conf = get_configuration(None).unwrap();
    let addr = conf.leptos_options.site_addr;
    let leptos_options = conf.leptos_options;
    let routes = generate_route_list(App);

    let app = Router::new()
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
