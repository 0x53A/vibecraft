/// Convert vt100::Color to 0xRRGGBB u32 for beamterm.
pub fn ansi_to_rgb(color: vt100::Color, is_foreground: bool) -> u32 {
    match color {
        vt100::Color::Default => {
            if is_foreground {
                0xCCCCCC // light gray foreground
            } else {
                0x1A1A2E // dark background
            }
        }
        vt100::Color::Idx(idx) => idx_to_rgb(idx),
        vt100::Color::Rgb(r, g, b) => ((r as u32) << 16) | ((g as u32) << 8) | (b as u32),
    }
}

/// Standard ANSI 256-color palette to RGB.
fn idx_to_rgb(idx: u8) -> u32 {
    match idx {
        // Standard 16 colors (bold variants handled by font style)
        0 => 0x000000,   // black
        1 => 0xCC0000,   // red
        2 => 0x4E9A06,   // green
        3 => 0xC4A000,   // yellow
        4 => 0x3465A4,   // blue
        5 => 0x75507B,   // magenta
        6 => 0x06989A,   // cyan
        7 => 0xD3D7CF,   // white
        8 => 0x555753,   // bright black
        9 => 0xEF2929,   // bright red
        10 => 0x8AE234,  // bright green
        11 => 0xFCE94F,  // bright yellow
        12 => 0x729FCF,  // bright blue
        13 => 0xAD7FA8,  // bright magenta
        14 => 0x34E2E2,  // bright cyan
        15 => 0xEEEEEC,  // bright white
        // 216-color cube (indices 16-231)
        16..=231 => {
            let idx = idx - 16;
            let b = idx % 6;
            let g = (idx / 6) % 6;
            let r = idx / 36;
            let to_val = |c: u8| -> u32 {
                if c == 0 { 0 } else { (55 + 40 * c as u32) }
            };
            (to_val(r) << 16) | (to_val(g) << 8) | to_val(b)
        }
        // Grayscale ramp (indices 232-255)
        232..=255 => {
            let val = 8 + 10 * (idx - 232) as u32;
            (val << 16) | (val << 8) | val
        }
    }
}
