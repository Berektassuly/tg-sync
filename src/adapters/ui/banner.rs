//! Cyberpunk/Neon ASCII banner with gradient (TG-SYNC).
//! Uses embedded ANSI Shadow font for a solid/filled look.

use crossterm::ExecutableCommand;
use crossterm::style::{Color, Print, ResetColor, SetForegroundColor};
use figlet_rs::FIGfont;
use std::io::{Write, stdout};

/// ANSI Shadow FLF font embedded at compile time (solid/filled style).
const ANSI_SHADOW_FONT: &str = include_str!("../../adapters/ui/fonts/ANSI_Shadow.flf");

/// Neon Purple (#bc13fe).
const NEON_PURPLE: (u8, u8, u8) = (0xbc, 0x13, 0xfe);
/// Cyber Green (#0ff0fc).
const CYBER_GREEN: (u8, u8, u8) = (0x0f, 0xf0, 0xfc);

/// Linear interpolation between two RGB colors. `t` in [0.0, 1.0].
fn lerp_rgb(a: (u8, u8, u8), b: (u8, u8, u8), t: f64) -> (u8, u8, u8) {
    let r = (f64::from(a.0) * (1.0 - t) + f64::from(b.0) * t).round() as u8;
    let g = (f64::from(a.1) * (1.0 - t) + f64::from(b.1) * t).round() as u8;
    let bl = (f64::from(a.2) * (1.0 - t) + f64::from(b.2) * t).round() as u8;
    (r, g, bl)
}

/// Prints the welcome banner: "TG-SYNC" in ANSI Shadow (solid/filled) ASCII with a gradient
/// from Neon Purple to Cyber Green, then version and "Powered by Rust".
pub fn print_welcome() {
    let mut out = stdout();
    let font = FIGfont::from_content(ANSI_SHADOW_FONT).expect("figlet ANSI Shadow font");
    let figure = font.convert("TG-SYNC").expect("figlet convert TG-SYNC");
    let art = figure.to_string();
    let lines: Vec<&str> = art.lines().collect();
    let total = lines.len().max(1);

    for (i, line) in lines.iter().enumerate() {
        let t = if total <= 1 {
            1.0
        } else {
            i as f64 / (total - 1) as f64
        };
        let (r, g, b) = lerp_rgb(NEON_PURPLE, CYBER_GREEN, t);
        let _ = out.execute(SetForegroundColor(Color::Rgb { r, g, b }));
        let _ = out.execute(Print(line));
        let _ = out.execute(Print("\r\n"));
        let _ = out.execute(ResetColor);
    }

    let version = env!("CARGO_PKG_VERSION");
    let _ = out.execute(SetForegroundColor(Color::Rgb {
        r: CYBER_GREEN.0,
        g: CYBER_GREEN.1,
        b: CYBER_GREEN.2,
    }));
    let _ = out.execute(Print(format!("v{}\r\n", version)));
    let _ = out.execute(Print("Powered by Rust\r\n"));
    let _ = out.execute(ResetColor);
    let _ = out.flush();
}
