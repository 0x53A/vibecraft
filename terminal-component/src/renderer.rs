use beamterm_renderer::{CellData, FontStyle, GlyphEffect};

use crate::colors::ansi_to_rgb;

// Thread-local string arena to avoid leaking memory each frame.
// We reuse a pool of Strings across frames.
thread_local! {
    static STRING_ARENA: std::cell::RefCell<Vec<String>> = std::cell::RefCell::new(Vec::new());
}

/// Map a vt100 Screen to a Vec of CellData for beamterm rendering.
/// Includes cursor rendering via color inversion.
///
/// The returned CellData borrows from the thread-local string arena.
/// Callers must use the result before the next call to this function.
pub fn screen_to_cells(screen: &vt100::Screen) -> Vec<CellData<'static>> {
    let (rows, cols) = screen.size();
    let cursor = screen.cursor_position();
    let show_cursor = !screen.hide_cursor();

    let total = rows as usize * cols as usize;

    // Reset the arena but keep capacity
    STRING_ARENA.with(|arena| {
        let mut arena = arena.borrow_mut();
        arena.clear();
        arena.reserve(total);
    });

    let mut cells = Vec::with_capacity(total);

    for row in 0..rows {
        for col in 0..cols {
            let cell = match screen.cell(row, col) {
                Some(c) => c,
                None => {
                    cells.push(CellData::new(
                        " ",
                        FontStyle::Normal,
                        GlyphEffect::None,
                        0xCCCCCC,
                        0x1A1A2E,
                    ));
                    continue;
                }
            };

            let contents = cell.contents();
            // Store the string in the arena and get a 'static reference.
            // SAFETY: The arena lives in a thread-local and is only cleared at
            // the start of the next screen_to_cells call. The CellData is consumed
            // by beamterm's update_cells before then.
            let symbol: &'static str = if contents.is_empty() {
                " "
            } else {
                STRING_ARENA.with(|arena| {
                    let mut arena = arena.borrow_mut();
                    arena.push(contents.to_string());
                    let s: &str = arena.last().unwrap().as_str();
                    // SAFETY: arena is not cleared until next screen_to_cells call
                    unsafe { std::mem::transmute::<&str, &'static str>(s) }
                })
            };

            let mut fg = ansi_to_rgb(cell.fgcolor(), true);
            let mut bg = ansi_to_rgb(cell.bgcolor(), false);

            // Inverse attribute
            if cell.inverse() {
                std::mem::swap(&mut fg, &mut bg);
            }

            // Cursor: invert colors at cursor position
            if show_cursor && row == cursor.0 && col == cursor.1 {
                std::mem::swap(&mut fg, &mut bg);
            }

            let style = match (cell.bold(), cell.italic()) {
                (true, true) => FontStyle::BoldItalic,
                (true, false) => FontStyle::Bold,
                (false, true) => FontStyle::Italic,
                (false, false) => FontStyle::Normal,
            };

            let effect = if cell.underline() {
                GlyphEffect::Underline
            } else {
                GlyphEffect::None
            };

            cells.push(CellData::new(symbol, style, effect, fg, bg));
        }
    }

    cells
}
