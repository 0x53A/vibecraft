use beamterm_renderer::{mouse::MouseSelectOptions, Terminal};
use rust_web_component::WebComponent;
use rust_web_component_macro::WebComponent;
use wasm_bindgen::prelude::*;
use web_sys::{HtmlCanvasElement, HtmlElement, KeyboardEvent, MessageEvent, WebSocket};

use crate::{input, renderer, websocket};

const DEFAULT_ROWS: u16 = 24;
const DEFAULT_COLS: u16 = 80;
const FONT_SIZE: f32 = 12.0;
const FONT_FAMILIES: &[&str] = &["JetBrains Mono", "Fira Code", "monospace"];

#[derive(WebComponent)]
#[web_component(name = "vibecraft-terminal", observed_attributes = ["session-id"])]
pub struct VibecraftTerminal {
    element: Option<HtmlElement>,
    terminal: Option<Terminal>,
    parser: Option<vt100::Parser>,
    ws: Option<WebSocket>,
    session_id: Option<String>,
    rows: u16,
    cols: u16,
    focused: bool,
    // Store closures to prevent them from being dropped
    _key_closure: Option<Closure<dyn FnMut(KeyboardEvent)>>,
    _ws_message_closure: Option<Closure<dyn FnMut(MessageEvent)>>,
    _ws_close_closure: Option<Closure<dyn FnMut(web_sys::Event)>>,
    _resize_closure: Option<Closure<dyn FnMut(js_sys::Array)>>,
}

impl VibecraftTerminal {
    pub fn new() -> Self {
        Self {
            element: None,
            terminal: None,
            parser: None,
            ws: None,
            session_id: None,
            rows: DEFAULT_ROWS,
            cols: DEFAULT_COLS,
            focused: false,
            _key_closure: None,
            _ws_message_closure: None,
            _ws_close_closure: None,
            _resize_closure: None,
        }
    }

    fn render_screen(&mut self) {
        let (terminal, parser) = match (self.terminal.as_mut(), self.parser.as_ref()) {
            (Some(t), Some(p)) => (t, p),
            _ => return,
        };

        let screen = parser.screen();
        let cells = renderer::screen_to_cells(screen);
        if let Err(e) = terminal.update_cells(cells.iter().copied()) {
            web_sys::console::error_1(&format!("update_cells error: {:?}", e).into());
            return;
        }
        if let Err(e) = terminal.render_frame() {
            web_sys::console::error_1(&format!("render_frame error: {:?}", e).into());
        }
    }

    fn connect_ws(&mut self) {
        // Close existing connection
        self.close_ws();

        let session_id = match &self.session_id {
            Some(id) => id.clone(),
            None => return,
        };

        // Determine base URL from the page
        let location = web_sys::window().unwrap().location();
        let base_url = format!(
            "{}//{}",
            location.protocol().unwrap_or_default(),
            location.host().unwrap_or_default()
        );

        let ws = match websocket::connect(&base_url, &session_id) {
            Ok(ws) => ws,
            Err(e) => {
                web_sys::console::error_1(&format!("WS connect error: {:?}", e).into());
                return;
            }
        };

        // Set up message handler
        let element = self.element.clone().unwrap();
        let on_message = Closure::wrap(Box::new(move |event: MessageEvent| {
            if let Some(bytes) = websocket::extract_bytes(&event) {
                // Call back into the component via the element reference
                VibecraftTerminal::with_element(&element, |comp| {
                    comp.feed_bytes(&bytes);
                });
            }
        }) as Box<dyn FnMut(MessageEvent)>);
        ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));

        // Open handler - send resize once connection is established
        let element_open = self.element.clone().unwrap();
        let on_open = Closure::wrap(Box::new(move |_event: web_sys::Event| {
            VibecraftTerminal::with_element(&element_open, |comp| {
                web_sys::console::log_1(
                    &format!("WS open, sending resize {}x{}", comp.cols, comp.rows).into(),
                );
                if let Some(ws) = &comp.ws {
                    websocket::send_resize(ws, comp.rows, comp.cols);
                }
            });
        }) as Box<dyn FnMut(web_sys::Event)>);
        ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        on_open.forget();

        // Close handler
        let on_close = Closure::wrap(Box::new(move |_event: web_sys::Event| {
            web_sys::console::log_1(&"Terminal WS closed".into());
        }) as Box<dyn FnMut(web_sys::Event)>);
        ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));

        self.ws = Some(ws);
        self._ws_message_closure = Some(on_message);
        self._ws_close_closure = Some(on_close);
    }

    fn close_ws(&mut self) {
        if let Some(ws) = self.ws.take() {
            let _ = ws.close();
        }
        self._ws_message_closure = None;
        self._ws_close_closure = None;
    }

    pub fn feed_bytes(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        // Each capture-pane output is a complete snapshot — reset parser.
        self.parser = Some(vt100::Parser::new(self.rows, self.cols, 0));

        if let Some(parser) = self.parser.as_mut() {
            // Strip trailing newline — it would scroll the first line off-screen
            let data = if bytes.last() == Some(&b'\n') {
                &bytes[..bytes.len() - 1]
            } else {
                bytes
            };

            // capture-pane uses bare \n; vt100 needs \r\n to return to column 0
            let mut processed = Vec::with_capacity(data.len() + data.len() / 10);
            for &b in data {
                if b == b'\n' {
                    processed.push(b'\r');
                }
                processed.push(b);
            }
            parser.process(&processed);
        }
        self.render_screen();
    }

    fn setup_keyboard(&mut self) {
        let element = match &self.element {
            Some(el) => el.clone(),
            None => return,
        };

        let el_for_closure = element.clone();
        let key_closure = Closure::wrap(Box::new(move |event: KeyboardEvent| {
            // Let browser handle its own shortcuts (Ctrl+Shift+I, Ctrl+L, etc.)
            // Only intercept keys we'll send to the terminal
            let dominated_by_browser = event.meta_key()
                || (event.ctrl_key() && event.shift_key());

            if dominated_by_browser {
                return;
            }

            VibecraftTerminal::with_element(&el_for_closure, |comp| {
                if let Some(seq) = input::key_event_to_terminal_seq(&event) {
                    event.prevent_default();
                    event.stop_propagation();
                    if let Some(ws) = &comp.ws {
                        websocket::send_input(ws, &seq);
                    }
                }
            });
        }) as Box<dyn FnMut(KeyboardEvent)>);

        // Attach to the shadow root's canvas or the element itself
        let _ = element.add_event_listener_with_callback(
            "keydown",
            key_closure.as_ref().unchecked_ref(),
        );

        // Make element focusable
        element
            .set_attribute("tabindex", "0")
            .unwrap_or_default();

        self._key_closure = Some(key_closure);
    }

    fn setup_resize_observer(&mut self) {
        let element = match &self.element {
            Some(el) => el.clone(),
            None => return,
        };

        let el_for_closure = element.clone();
        let resize_closure = Closure::wrap(Box::new(move |entries: js_sys::Array| {
            let entry: web_sys::ResizeObserverEntry = entries.get(0).unchecked_into();
            let rect = entry.content_rect();
            let width = rect.width() as f32;
            let height = rect.height() as f32;

            VibecraftTerminal::with_element(&el_for_closure, |comp| {
                comp.handle_resize(width, height);
            });
        }) as Box<dyn FnMut(js_sys::Array)>);

        let observer =
            web_sys::ResizeObserver::new(resize_closure.as_ref().unchecked_ref()).unwrap();
        observer.observe(&element);

        self._resize_closure = Some(resize_closure);
        // Note: observer is not stored — it will be GC'd when element is removed
        // which is fine since disconnected() handles cleanup
        std::mem::forget(observer); // prevent drop, let JS GC handle it
    }

    fn handle_resize(&mut self, width: f32, height: f32) {
        if width <= 0.0 || height <= 0.0 {
            return;
        }

        let terminal = match self.terminal.as_mut() {
            Some(t) => t,
            None => return,
        };

        // Resize beamterm first, then query its actual grid size
        if let Err(e) = terminal.resize(width as i32, height as i32) {
            web_sys::console::error_1(&format!("resize error: {:?}", e).into());
            return;
        }

        let size = terminal.terminal_size();
        let new_rows = size.rows;
        let new_cols = size.cols;

        if new_cols < 2 || new_rows < 2 {
            return;
        }

        if new_cols != self.cols || new_rows != self.rows {
            self.cols = new_cols;
            self.rows = new_rows;

            web_sys::console::log_1(
                &format!("resize: {}x{}", new_cols, new_rows).into(),
            );

            // Resize vt100 parser to match beamterm exactly
            if let Some(parser) = self.parser.as_mut() {
                parser.screen_mut().set_size(new_rows, new_cols);
            }

            // Notify server to resize tmux pane
            if let Some(ws) = &self.ws {
                websocket::send_resize(ws, new_rows, new_cols);
            }

            self.render_screen();
        }
    }
}

impl WebComponent for VibecraftTerminal {
    fn attach(&mut self, element: &HtmlElement) {
        self.element = Some(element.clone());
    }

    fn connected(&mut self) {
        let element = self.element.as_ref().unwrap();
        let document = web_sys::window().unwrap().document().unwrap();

        // Create shadow DOM (reuse existing if element was re-inserted)
        let shadow = match element.shadow_root() {
            Some(existing) => {
                // Clear previous contents
                existing.set_inner_html("");
                existing
            }
            None => element
                .attach_shadow(&web_sys::ShadowRootInit::new(web_sys::ShadowRootMode::Open))
                .expect("failed to attach shadow root"),
        };

        // Add styles
        let style = document.create_element("style").unwrap();
        style.set_text_content(Some(
            ":host { display: block; width: 100%; height: 100%; overflow: hidden; background: #1A1A2E; }
             canvas { display: block; width: 100%; height: 100%; outline: none; }
             :host(:focus-within) { box-shadow: 0 0 0 1px #E85D26; }"
        ));
        shadow.append_child(&style).unwrap();

        // Create canvas
        let canvas = document
            .create_element("canvas")
            .expect("failed to create canvas")
            .unchecked_into::<HtmlCanvasElement>();

        shadow.append_child(&canvas).unwrap();

        // Initialize beamterm terminal
        let terminal_result = Terminal::builder(canvas)
            .dynamic_font_atlas(FONT_FAMILIES, FONT_SIZE)
            .mouse_selection_handler(
                MouseSelectOptions::new()
                    .selection_mode(beamterm_renderer::SelectionMode::Linear),
            )
            .build();

        match terminal_result {
            Ok(terminal) => {
                // Sync vt100 parser size with beamterm's actual grid size
                let size = terminal.terminal_size();
                self.rows = size.rows;
                self.cols = size.cols;
                web_sys::console::log_1(
                    &format!("beamterm grid: {}x{}", self.cols, self.rows).into(),
                );
                self.terminal = Some(terminal);
            }
            Err(e) => {
                web_sys::console::error_1(
                    &format!("Failed to init beamterm terminal: {:?}", e).into(),
                );
                return;
            }
        }

        // Initialize vt100 parser with beamterm's actual grid size
        self.parser = Some(vt100::Parser::new(self.rows, self.cols, 1000));

        // Set up keyboard input
        self.setup_keyboard();

        // Set up resize observer
        self.setup_resize_observer();

        // Initial render (blank screen)
        self.render_screen();

        // If session-id was set before connection, connect now
        if self.session_id.is_some() {
            self.connect_ws();
        }
    }

    fn disconnected(&mut self) {
        self.close_ws();
        self.terminal = None;
        self.parser = None;
        self._key_closure = None;
        self._resize_closure = None;
    }

    fn attribute_changed(&mut self, name: &str, _old: Option<&str>, new: Option<&str>) {
        match name {
            "session-id" => {
                self.session_id = new.map(String::from);
                if self.terminal.is_some() {
                    // Reset parser for new session
                    self.parser = Some(vt100::Parser::new(self.rows, self.cols, 1000));
                    self.connect_ws();
                }
            }
            _ => {}
        }
    }
}
