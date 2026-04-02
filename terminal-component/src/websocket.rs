use serde::Serialize;
use wasm_bindgen::prelude::*;
use web_sys::{MessageEvent, WebSocket};

#[derive(Serialize)]
#[serde(tag = "type")]
pub enum ClientMessage {
    #[serde(rename = "input")]
    Input { data: String },
    #[serde(rename = "resize")]
    Resize { rows: u16, cols: u16 },
}

/// Create a WebSocket connection to the terminal stream endpoint.
/// Returns the WebSocket handle. Callers attach onmessage/onclose handlers.
pub fn connect(base_url: &str, session_id: &str) -> Result<WebSocket, JsValue> {
    // Derive WS URL from current page location
    let ws_url = if base_url.starts_with("https") {
        format!(
            "wss://{}/sessions/{}/terminal-stream",
            &base_url[8..], // strip "https://"
            session_id
        )
    } else if base_url.starts_with("http") {
        format!(
            "ws://{}/sessions/{}/terminal-stream",
            &base_url[7..], // strip "http://"
            session_id
        )
    } else {
        // Relative — use window location
        let location = web_sys::window().unwrap().location();
        let protocol = location.protocol().unwrap_or_default();
        let host = location.host().unwrap_or_default();
        let ws_proto = if protocol == "https:" { "wss" } else { "ws" };
        format!(
            "{}://{}/sessions/{}/terminal-stream",
            ws_proto, host, session_id
        )
    };

    let ws = WebSocket::new(&ws_url)?;
    ws.set_binary_type(web_sys::BinaryType::Arraybuffer);
    Ok(ws)
}

/// Send an input message (keyboard data) over the WebSocket.
pub fn send_input(ws: &WebSocket, data: &str) {
    let msg = ClientMessage::Input {
        data: data.to_string(),
    };
    if let Ok(json) = serde_json::to_string(&msg) {
        let _ = ws.send_with_str(&json);
    }
}

/// Send a resize message over the WebSocket.
pub fn send_resize(ws: &WebSocket, rows: u16, cols: u16) {
    let msg = ClientMessage::Resize { rows, cols };
    if let Ok(json) = serde_json::to_string(&msg) {
        let _ = ws.send_with_str(&json);
    }
}

/// Extract bytes from a WebSocket MessageEvent.
/// Handles both ArrayBuffer and string messages.
pub fn extract_bytes(event: &MessageEvent) -> Option<Vec<u8>> {
    let data = event.data();

    // Try ArrayBuffer first
    if let Ok(buffer) = data.dyn_into::<js_sys::ArrayBuffer>() {
        let array = js_sys::Uint8Array::new(&buffer);
        return Some(array.to_vec());
    }

    // Fall back to string
    let data = event.data();
    if let Some(text) = data.as_string() {
        return Some(text.into_bytes());
    }

    None
}
