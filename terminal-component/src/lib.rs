mod colors;
mod component;
mod input;
mod renderer;
mod websocket;

use wasm_bindgen::prelude::*;

pub use component::VibecraftTerminal;

/// Initialize the vibecraft-terminal web component.
/// Call this once after loading the WASM module.
/// Safe to call multiple times (e.g., HMR reloads).
#[wasm_bindgen(start)]
pub fn init() {
    console_log::init_with_level(log::Level::Debug).ok();
    console_error_panic_hook::set_once();

    // Guard against double registration (HMR, multiple script loads)
    let window = web_sys::window().unwrap();
    let ce = window
        .custom_elements();
    if ce.get("vibecraft-terminal").is_truthy() {
        log::debug!("vibecraft-terminal already registered, skipping");
        return;
    }

    VibecraftTerminal::setup();
    log::info!("vibecraft-terminal web component registered");
}

/// Feed raw terminal bytes to a vibecraft-terminal element.
/// Called from JS when data arrives (e.g., from fetch).
#[wasm_bindgen]
pub fn terminal_feed(element: &web_sys::HtmlElement, data: &[u8]) {
    VibecraftTerminal::with_element(element, |comp| {
        comp.feed_bytes(data);
    });
}
