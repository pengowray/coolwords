// Leptos' deeply nested view! trees push the trait solver past its default depth
// under the release profile (LTO + codegen-units=1); lift the limit so the wasm
// build resolves. Inner attribute → must precede all items in the crate root.
#![recursion_limit = "1024"]

pub mod app;
pub mod booktags;
pub mod catalog;
#[cfg(feature = "ssr")]
pub mod jobs;

#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn hydrate() {
    use crate::app::*;
    console_error_panic_hook::set_once();
    // Under Home Assistant ingress the browser sits at `<origin>/api/hassio_ingress/
    // <token>/`, so a server-function POST to the default absolute `/api/...` would
    // miss the ingress route and 404. Point server-fn calls at `<origin><base>` so HA
    // strips the prefix back off and routes them to us. No-op off ingress (base == "").
    let base = ingress_base();
    if !base.is_empty() {
        let origin = web_sys::window()
            .and_then(|w| w.location().origin().ok())
            .unwrap_or_default();
        server_fn::client::set_server_url(format!("{origin}{base}").leak());
    }
    leptos::mount::hydrate_body(App);
}
