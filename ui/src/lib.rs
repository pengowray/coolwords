// Leptos' deeply nested view! trees push the trait solver past its default depth
// under the release profile (LTO + codegen-units=1); lift the limit so the wasm
// build resolves. Inner attribute → must precede all items in the crate root.
#![recursion_limit = "512"]

pub mod app;
#[cfg(feature = "ssr")]
pub mod jobs;

#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn hydrate() {
    use crate::app::*;
    console_error_panic_hook::set_once();
    leptos::mount::hydrate_body(App);
}
