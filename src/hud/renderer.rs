///////////////////////////////////////////////////////////////////////////////
//  renderer.rs  —  Dashcam HUD  ·  Gran Turismo × Cyberpunk 2077
//
//  Layout on a transparent 4K canvas:
//
//  ┌────────────────────────────────────────────────────────────────────────┐
//  │ [G-FORCE RADAR]        [COMPASS BAR]              [GPS TAG]           │
//  │  top-left               top-center                 top-right          │
//  │                                                                        │
//  │ [SPEEDOMETER]                                     [MINI-MAP]          │
//  │  bottom-left                                       bottom-right       │
//  └────────────────────────────────────────────────────────────────────────┘
//
//  Every widget:
//    • Dark-glass panel  (BG_PANEL + scanlines)
//    • Chamfered corner brackets in NEON_CYAN
//    • Multi-pass glow on all active elements
///////////////////////////////////////////////////////////////////////////////

use image::{Rgba, RgbaImage};
use imageproc::drawing::{
    draw_antialiased_line_segment_mut, draw_filled_circle_mut, draw_hollow_circle_mut,
};
use imageproc::pixelops::interpolate as aa_interp;
use std::f64::consts::PI;

use crate::hud::elements::HudElements;
use crate::telemetry::vantrue_frames::{AccelerometerFrame, TelemetryFrame};

// ═══════════════════════════════════════════════════════════════════════════════
// PALETTE
// ═══════════════════════════════════════════════════════════════════════════════
const BG_PANEL:       Rgba<u8> = Rgba([4,   10,  24, 200]);
const NEON_CYAN:      Rgba<u8> = Rgba([0,   230, 255, 255]);
const NEON_CYAN_DIM:  Rgba<u8> = Rgba([0,   110, 145, 155]);
const NEON_CYAN_GLOW: Rgba<u8> = Rgba([0,   200, 255,  40]);
const NEON_ORANGE:    Rgba<u8> = Rgba([255, 148,   0, 255]);
const NEON_RED:       Rgba<u8> = Rgba([255,  28,  55, 255]);
const NEON_GREEN:     Rgba<u8> = Rgba([0,   255, 120, 255]);
const NEON_YELLOW:    Rgba<u8> = Rgba([255, 232,   0, 255]);
const WHITE:          Rgba<u8> = Rgba([218, 234, 255, 255]);
const GREY:           Rgba<u8> = Rgba([78,   94, 118, 200]);
const DARK_GREY:      Rgba<u8> = Rgba([18,   26,  44, 255]);
const SCANLINE:       Rgba<u8> = Rgba([0,    10,  28,  32]);

// ═══════════════════════════════════════════════════════════════════════════════
// LOW-LEVEL DRAWING PRIMITIVES
// ═══════════════════════════════════════════════════════════════════════════════

#[inline(always)]
fn safe_put(img: &mut RgbaImage, x: u32, y: u32, c: Rgba<u8>) {
    if x < img.width() && y < img.height() {
        img.put_pixel(x, y, c);
    }
}

#[inline(always)]
fn alpha_blend(dst: Rgba<u8>, src: Rgba<u8>) -> Rgba<u8> {
    let a = src[3] as u32;
    if a == 0   { return dst; }
    if a == 255 { return src; }
    let ia = 255 - a;
    let oa = (a + dst[3] as u32 * ia / 255).min(255) as u8;
    Rgba([
        ((src[0] as u32 * a + dst[0] as u32 * ia) / 255) as u8,
        ((src[1] as u32 * a + dst[1] as u32 * ia) / 255) as u8,
        ((src[2] as u32 * a + dst[2] as u32 * ia) / 255) as u8,
        oa,
    ])
}

#[inline(always)]
fn bput(img: &mut RgbaImage, x: u32, y: u32, c: Rgba<u8>) {
    if x < img.width() && y < img.height() {
        let d = *img.get_pixel(x, y);
        img.put_pixel(x, y, alpha_blend(d, c));
    }
}

fn fill_rect(img: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32, c: Rgba<u8>) {
    for dy in 0..h {
        for dx in 0..w {
            bput(img, x + dx, y + dy, c);
        }
    }
}

fn line(img: &mut RgbaImage, x0: i32, y0: i32, x1: i32, y1: i32, c: Rgba<u8>) {
    draw_antialiased_line_segment_mut(img, (x0, y0), (x1, y1), c, aa_interp);
}

/// Anti-aliased line with a soft radial glow halo
fn glow_line(img: &mut RgbaImage, x0: i32, y0: i32, x1: i32, y1: i32, c: Rgba<u8>, glow_r: i32) {
    for r in 1..=glow_r {
        let a = ((c[3] as i32) / (r * 2 + 1)).clamp(0, 255) as u8;
        let gc = Rgba([c[0], c[1], c[2], a]);
        for off in [-r, r] {
            draw_antialiased_line_segment_mut(img, (x0+off, y0), (x1+off, y1), gc, aa_interp);
            draw_antialiased_line_segment_mut(img, (x0, y0+off), (x1, y1+off), gc, aa_interp);
        }
    }
    draw_antialiased_line_segment_mut(img, (x0, y0), (x1, y1), c, aa_interp);
}

fn glow_circle(img: &mut RgbaImage, cx: i32, cy: i32, r: i32, c: Rgba<u8>) {
    for dr in [-3i32, -2, -1, 0, 1, 2, 3] {
        let a = (c[3] as i32 - dr.unsigned_abs() as i32 * 45).clamp(0, 255) as u8;
        draw_hollow_circle_mut(img, (cx, cy), (r + dr).max(1), Rgba([c[0], c[1], c[2], a]));
    }
}

fn glow_dot(img: &mut RgbaImage, cx: i32, cy: i32, r: i32, c: Rgba<u8>) {
    draw_filled_circle_mut(img, (cx, cy), r + 6, Rgba([c[0], c[1], c[2], 25]));
    draw_filled_circle_mut(img, (cx, cy), r + 4, Rgba([c[0], c[1], c[2], 55]));
    draw_filled_circle_mut(img, (cx, cy), r + 2, Rgba([c[0], c[1], c[2], 100]));
    draw_filled_circle_mut(img, (cx, cy), r,     c);
}

// ═══════════════════════════════════════════════════════════════════════════════
// BITMAP FONT  (5 × 9 px per glyph, scaled with `scale`)
// ═══════════════════════════════════════════════════════════════════════════════
const FONT_W: u32 = 5;
const FONT_H: u32 = 9;

fn char_bm(ch: char) -> [u8; 9] {
    match ch {
        '0' => [0b01110,0b10001,0b10001,0b10011,0b10101,0b11001,0b10001,0b10001,0b01110],
        '1' => [0b00100,0b01100,0b10100,0b00100,0b00100,0b00100,0b00100,0b00100,0b11111],
        '2' => [0b01110,0b10001,0b00001,0b00010,0b00100,0b01000,0b10000,0b10000,0b11111],
        '3' => [0b11111,0b00010,0b00100,0b00010,0b00001,0b00001,0b00001,0b10001,0b01110],
        '4' => [0b00010,0b00110,0b01010,0b10010,0b10010,0b11111,0b00010,0b00010,0b00010],
        '5' => [0b11111,0b10000,0b10000,0b11110,0b00001,0b00001,0b00001,0b10001,0b01110],
        '6' => [0b00110,0b01000,0b10000,0b10000,0b11110,0b10001,0b10001,0b10001,0b01110],
        '7' => [0b11111,0b00001,0b00001,0b00010,0b00100,0b01000,0b01000,0b01000,0b01000],
        '8' => [0b01110,0b10001,0b10001,0b10001,0b01110,0b10001,0b10001,0b10001,0b01110],
        '9' => [0b01110,0b10001,0b10001,0b10001,0b01111,0b00001,0b00001,0b00010,0b01100],
        '.' => [0,0,0,0,0,0,0,0b01100,0b01100],
        ':' => [0,0b01100,0b01100,0,0,0b01100,0b01100,0,0],
        '-' => [0,0,0,0,0b11111,0,0,0,0],
        '+' => [0,0b00100,0b00100,0b11111,0b00100,0b00100,0,0,0],
        '/' => [0b00001,0b00010,0b00100,0b01000,0b10000,0,0,0,0],
        ' ' => [0;9],
        // ── Uppercase ──────────────────────────────────────────────────────
        'A' => [0b01110,0b10001,0b10001,0b11111,0b10001,0b10001,0b10001,0b10001,0b10001],
        'B' => [0b11110,0b10001,0b10001,0b11110,0b10001,0b10001,0b10001,0b10001,0b11110],
        'C' => [0b01110,0b10001,0b10000,0b10000,0b10000,0b10000,0b10000,0b10001,0b01110],
        'D' => [0b11100,0b10010,0b10001,0b10001,0b10001,0b10001,0b10001,0b10010,0b11100],
        'E' => [0b11111,0b10000,0b10000,0b11110,0b10000,0b10000,0b10000,0b10000,0b11111],
        'F' => [0b11111,0b10000,0b10000,0b11110,0b10000,0b10000,0b10000,0b10000,0b10000],
        'G' => [0b01110,0b10001,0b10000,0b10111,0b10001,0b10001,0b10001,0b10001,0b01111],
        'H' => [0b10001,0b10001,0b10001,0b11111,0b10001,0b10001,0b10001,0b10001,0b10001],
        'I' => [0b01110,0b00100,0b00100,0b00100,0b00100,0b00100,0b00100,0b00100,0b01110],
        'J' => [0b00111,0b00010,0b00010,0b00010,0b00010,0b00010,0b10010,0b10010,0b01100],
        'K' => [0b10001,0b10010,0b10100,0b11000,0b11000,0b10100,0b10010,0b10001,0b10001],
        'L' => [0b10000,0b10000,0b10000,0b10000,0b10000,0b10000,0b10000,0b10000,0b11111],
        'M' => [0b10001,0b11011,0b10101,0b10101,0b10001,0b10001,0b10001,0b10001,0b10001],
        'N' => [0b10001,0b11001,0b10101,0b10011,0b10001,0b10001,0b10001,0b10001,0b10001],
        'O' => [0b01110,0b10001,0b10001,0b10001,0b10001,0b10001,0b10001,0b10001,0b01110],
        'P' => [0b11110,0b10001,0b10001,0b11110,0b10000,0b10000,0b10000,0b10000,0b10000],
        'Q' => [0b01110,0b10001,0b10001,0b10001,0b10001,0b10101,0b10011,0b10001,0b01111],
        'R' => [0b11110,0b10001,0b10001,0b11110,0b11000,0b10100,0b10010,0b10001,0b10001],
        'S' => [0b01111,0b10000,0b10000,0b10000,0b01110,0b00001,0b00001,0b00001,0b11110],
        'T' => [0b11111,0b00100,0b00100,0b00100,0b00100,0b00100,0b00100,0b00100,0b00100],
        'U' => [0b10001,0b10001,0b10001,0b10001,0b10001,0b10001,0b10001,0b10001,0b01110],
        'V' => [0b10001,0b10001,0b10001,0b10001,0b01010,0b01010,0b01010,0b00100,0b00100],
        'W' => [0b10001,0b10001,0b10001,0b10101,0b10101,0b10101,0b01010,0b01010,0b01010],
        'X' => [0b10001,0b10001,0b01010,0b00100,0b00100,0b01010,0b10001,0b10001,0b10001],
        'Y' => [0b10001,0b10001,0b01010,0b00100,0b00100,0b00100,0b00100,0b00100,0b00100],
        'Z' => [0b11111,0b00001,0b00010,0b00100,0b01000,0b10000,0b10000,0b10000,0b11111],
        // ── Lowercase ──────────────────────────────────────────────────────
        'h' => [0b10000,0b10000,0b10110,0b11001,0b10001,0b10001,0b10001,0b10001,0b10001],
        'k' => [0b10000,0b10000,0b10010,0b10100,0b11000,0b11000,0b10100,0b10010,0b10001],
        'm' => [0,0,0b11010,0b10101,0b10101,0b10001,0b10001,0b10001,0b10001],
        _   => [0b11111,0b10001,0b10001,0b10001,0b10001,0b10001,0b10001,0b10001,0b11111],
    }
}

pub fn draw_text(img: &mut RgbaImage, text: &str, x: u32, y: u32, scale: u32, color: Rgba<u8>) {
    let cw = (FONT_W + 1) * scale;
    for (i, ch) in text.chars().enumerate() {
        let bm = char_bm(ch);
        let cx = x + i as u32 * cw;
        for (row, &bits) in bm.iter().enumerate() {
            for col in 0..FONT_W {
                if bits & (1 << (FONT_W - 1 - col)) != 0 {
                    for sy in 0..scale {
                        for sx in 0..scale {
                            safe_put(img, cx + col*scale + sx, y + row as u32*scale + sy, color);
                        }
                    }
                }
            }
        }
    }
}

fn tw(text: &str, scale: u32) -> u32 { text.chars().count() as u32 * (FONT_W + 1) * scale }
fn th(scale: u32) -> u32 { FONT_H * scale }

fn draw_text_c(img: &mut RgbaImage, text: &str, cx: u32, y: u32, scale: u32, color: Rgba<u8>) {
    draw_text(img, text, cx.saturating_sub(tw(text, scale) / 2), y, scale, color);
}

// ═══════════════════════════════════════════════════════════════════════════════
// PANEL FRAME  (glass + scanlines + corner brackets)
// ═══════════════════════════════════════════════════════════════════════════════

fn corner_bracket(img: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32, len: u32, c: Rgba<u8>) {
    let r = x + w.saturating_sub(1);
    let b = y + h.saturating_sub(1);
    for i in 0..len {
        bput(img, x+i,   y,   c); bput(img, x,   y+i,   c);
        bput(img, r-i,   y,   c); bput(img, r,   y+i,   c);
        bput(img, x+i,   b,   c); bput(img, x,   b-i,   c);
        bput(img, r-i,   b,   c); bput(img, r,   b-i,   c);
    }
    // accent pixel at exact corners
    for dy in 0..2 { for dx in 0..2 {
        bput(img, x+dx,  y+dy,  c); bput(img, r-dx, y+dy,  c);
        bput(img, x+dx,  b-dy,  c); bput(img, r-dx, b-dy,  c);
    }}
}

/// Outer glow border around the bracket (1-pixel halo of NEON_CYAN_GLOW)
fn bracket_glow(img: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32, len: u32) {
    // just draw a slightly-larger bracket with glow colour
    if x >= 2 && y >= 2 {
        corner_bracket(img, x-2, y-2, w+4, h+4, len, NEON_CYAN_GLOW);
        corner_bracket(img, x-1, y-1, w+2, h+2, len, NEON_CYAN_GLOW);
    }
}

fn draw_panel(img: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32, label: &str) {
    // Glass fill
    fill_rect(img, x, y, w, h, BG_PANEL);

    // CRT scanlines every 4 rows
    let mut row = y + 2;
    while row < y + h {
        for dx in 0..w { bput(img, x+dx, row, SCANLINE); }
        row += 4;
    }

    // Top accent bar
    for dx in 0..w {
        bput(img, x+dx, y,   NEON_CYAN_DIM);
        bput(img, x+dx, y+1, Rgba([0, 55, 88, 45]));
    }

    bracket_glow(img, x, y, w, h, 24);
    corner_bracket(img, x, y, w, h, 24, NEON_CYAN);

    if !label.is_empty() {
        draw_text(img, label, x+6, y+6, 1, NEON_CYAN_DIM);
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// SPEEDOMETER
// ═══════════════════════════════════════════════════════════════════════════════

/// Draw a segmented arc: the track is made of small dash-segments, not a
/// continuous line — gives a more digital / game-like look.
pub fn draw_speedometer(
    img: &mut RgbaImage,
    speed_kmh: f64,
    max_speed: f64,
    cx: i32,
    cy: i32,
    radius: i32,
) {
    let pad = 34i32;
    let px = (cx - radius - pad).max(0) as u32;
    let py = (cy - radius - pad).max(0) as u32;
    let pw = (radius * 2 + pad * 2) as u32;
    let ph = (radius * 2 + pad * 2) as u32;

    draw_panel(img, px, py, pw, ph, "SPEED");

    // Angular extents: 270° sweep, starting at 135° (bottom-left)
    let a0 = PI * 0.75;
    let sweep = PI * 1.5;      // 270°

    let t = (speed_kmh / max_speed).clamp(0.0, 1.0);
    let redline_t = 0.82;      // redline starts at 82% of max

    // ── Segmented background track ──
    let n_segs = 60i32;        // 60 segments = one per 4.5°
    let seg_gap_deg = 2.0f64;
    for s in 0..n_segs {
        let frac = s as f64 / n_segs as f64;
        let angle = a0 + sweep * frac;
        let gap_r = (seg_gap_deg / 2.0).to_radians();
        let a_from = angle + gap_r;
        let a_to   = a0 + sweep * ((s + 1) as f64 / n_segs as f64) - gap_r;

        let seg_color = if frac >= redline_t {
            Rgba([60, 10, 18, 140])   // redline zone background: dark red
        } else {
            Rgba([18, 26, 48, 160])   // normal zone background: dark blue
        };

        draw_arc_segment(img, cx, cy, radius - 12, radius + 2, a_from, a_to, seg_color);
    }

    // ── Filled active segments ──
    let active_segs = (t * n_segs as f64) as i32;
    for s in 0..active_segs.min(n_segs) {
        let frac = s as f64 / n_segs as f64;
        let angle = a0 + sweep * frac;
        let gap_r = (seg_gap_deg / 2.0).to_radians();
        let a_from = angle + gap_r;
        let a_to   = a0 + sweep * ((s + 1) as f64 / n_segs as f64) - gap_r;

        let seg_color = arc_color(frac);
        // outer glow
        let gc = Rgba([seg_color[0], seg_color[1], seg_color[2], 60]);
        draw_arc_segment(img, cx, cy, radius - 18, radius + 8, a_from, a_to, gc);
        // main segment
        draw_arc_segment(img, cx, cy, radius - 12, radius + 2, a_from, a_to, seg_color);
    }

    // ── Redline zone highlight (even when not active) ──
    let rl_from = (redline_t * n_segs as f64) as i32;
    for s in rl_from..n_segs {
        let frac = s as f64 / n_segs as f64;
        let angle = a0 + sweep * frac;
        let gap_r = (seg_gap_deg / 2.0).to_radians();
        let a_from = angle + gap_r;
        let a_to   = a0 + sweep * ((s + 1) as f64 / n_segs as f64) - gap_r;
        // subtle redline dim marker
        draw_arc_segment(img, cx, cy, radius + 4, radius + 10, a_from, a_to, Rgba([120, 18, 30, 120]));
    }

    // ── Tick marks ──
    for tick in 0..=10i32 {
        let frac  = tick as f64 / 10.0;
        let angle = a0 + sweep * frac;
        let major = tick % 2 == 0;
        let inner = if major { radius - 30 } else { radius - 18 };
        let outer = radius + 2;
        let x0 = (cx as f64 + inner as f64 * angle.cos()) as i32;
        let y0 = (cy as f64 + inner as f64 * angle.sin()) as i32;
        let x1 = (cx as f64 + outer as f64 * angle.cos()) as i32;
        let y1 = (cy as f64 + outer as f64 * angle.sin()) as i32;

        let tc = if frac >= redline_t { NEON_RED } else if major { WHITE } else { GREY };
        glow_line(img, x0, y0, x1, y1, tc, if major { 2 } else { 1 });

        if major {
            let v = (max_speed * frac).round() as u32;
            let lbl = v.to_string();
            let lx = (cx as f64 + (inner - 16) as f64 * angle.cos()) as i32 - lbl.len() as i32 * 3;
            let ly = (cy as f64 + (inner - 16) as f64 * angle.sin()) as i32 - 4;
            draw_text(img, &lbl, lx.max(0) as u32, ly.max(0) as u32, 1,
                      if frac >= redline_t { Rgba([200, 50, 60, 200]) } else { GREY });
        }
    }

    // ── Needle ──
    let needle_angle = a0 + sweep * t;
    let tip_x  = (cx as f64 + (radius - 24) as f64 * needle_angle.cos()) as i32;
    let tip_y  = (cy as f64 + (radius - 24) as f64 * needle_angle.sin()) as i32;
    let base_x = (cx as f64 - 32.0 * needle_angle.cos()) as i32;
    let base_y = (cy as f64 - 32.0 * needle_angle.sin()) as i32;
    let nc = arc_color(t);
    glow_line(img, base_x, base_y, tip_x, tip_y, nc, 7);
    line(img, base_x, base_y, tip_x, tip_y, WHITE);

    // Hub
    draw_filled_circle_mut(img, (cx, cy), 18, DARK_GREY);
    glow_circle(img, cx, cy, 16, nc);
    draw_filled_circle_mut(img, (cx, cy), 8, WHITE);
    draw_filled_circle_mut(img, (cx, cy), 4, DARK_GREY);

    // ── Large digital speed readout ──
    let spd_str = format!("{:3.0}", speed_kmh);
    let ns = 5u32;
    let nx = cx as u32 - tw(&spd_str, ns) / 2;
    let ny = (cy + radius / 3 + 10) as u32;
    // shadow
    draw_text(img, &spd_str, nx+2, ny+2, ns, Rgba([0,0,0,170]));
    // glow
    draw_text(img, &spd_str, nx.saturating_sub(1), ny.saturating_sub(1), ns, Rgba([nc[0],nc[1],nc[2],70]));
    draw_text(img, &spd_str, nx+1, ny+1, ns, Rgba([nc[0],nc[1],nc[2],70]));
    // core
    draw_text(img, &spd_str, nx, ny, ns, WHITE);

    // unit
    draw_text_c(img, "km/h", cx as u32, ny + th(ns) + 6, 2, GREY);

    // ── Bottom linear progress bar ──
    let bx = px + 20;
    let by = py + ph - 26;
    let bw = pw - 40;
    fill_rect(img, bx, by, bw, 8, Rgba([10, 16, 34, 200]));
    let fw = (t * bw as f64) as u32;
    if fw > 0 { fill_rect(img, bx, by, fw, 8, nc); }
    // inner glow above bar
    if fw > 0 { fill_rect(img, bx, by.saturating_sub(1), fw, 1, Rgba([nc[0],nc[1],nc[2],80])); }
    corner_bracket(img, bx.saturating_sub(3), by.saturating_sub(3), bw+6, 14, 5, NEON_CYAN_DIM);
}

/// Draw a filled arc sector between two angles as individual pixel dots.
/// Fills all radii from r_inner to r_outer.
fn draw_arc_segment(
    img: &mut RgbaImage,
    cx: i32, cy: i32,
    r_inner: i32, r_outer: i32,
    a_from: f64, a_to: f64,
    color: Rgba<u8>,
) {
    // Step in degrees that guarantees ~1px spacing at outer radius
    let steps = ((r_outer as f64 * (a_to - a_from).abs()) as i32).max(4);
    for s in 0..=steps {
        let angle = a_from + (a_to - a_from) * s as f64 / steps as f64;
        for r in r_inner..=r_outer {
            let x = cx as f64 + r as f64 * angle.cos();
            let y = cy as f64 + r as f64 * angle.sin();
            bput(img, x as u32, y as u32, color);
        }
    }
}

fn arc_color(t: f64) -> Rgba<u8> {
    let t = t.clamp(0.0, 1.0);
    if t < 0.4 {
        // cyan → teal-green
        let u = t / 0.4;
        Rgba([0, (230.0 - 30.0 * u) as u8, (255.0 - 140.0 * u) as u8, 245])
    } else if t < 0.75 {
        // green → yellow
        let u = (t - 0.4) / 0.35;
        Rgba([(255.0 * u) as u8, (200.0 + 30.0 * (1.0-u)) as u8, 0, 245])
    } else {
        // yellow → red, intensifying
        let u = (t - 0.75) / 0.25;
        Rgba([255, (230.0 * (1.0 - u)) as u8, 0, 255])
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// G-FORCE RADAR
// ═══════════════════════════════════════════════════════════════════════════════

pub fn draw_gforce(
    img: &mut RgbaImage,
    accel: &AccelerometerFrame,
    cx: i32,
    cy: i32,
    radius: i32,
    history: &[(f32, f32)],
) {
    let pad = 30i32;
    let px = (cx - radius - pad).max(0) as u32;
    let py = (cy - radius - pad).max(0) as u32;
    let pw = (radius * 2 + pad * 2) as u32;
    let ph = (radius * 2 + pad * 2 + 38) as u32;

    draw_panel(img, px, py, pw, ph, "G-FORCE");

    // ── Concentric rings with gradient shading ──
    for (i, &rr) in [radius/3, 2*radius/3, radius].iter().enumerate() {
        let a = [70u8, 100, 135][i];
        glow_circle(img, cx, cy, rr, Rgba([0, 80, 130, a]));
        let lbl = ["0.3", "0.6", "1.0"][i];
        draw_text(img, lbl, (cx + rr + 3) as u32, cy as u32 - 4, 1, Rgba([0, 75, 115, 140]));
    }

    // Diagonal guide lines (45° cross)
    for angle_deg in [0, 45, 90, 135] {
        let angle = (angle_deg as f64).to_radians();
        let x0 = (cx as f64 - radius as f64 * angle.cos()) as i32;
        let y0 = (cy as f64 - radius as f64 * angle.sin()) as i32;
        let x1 = (cx as f64 + radius as f64 * angle.cos()) as i32;
        let y1 = (cy as f64 + radius as f64 * angle.sin()) as i32;
        draw_antialiased_line_segment_mut(img, (x0,y0),(x1,y1), Rgba([15,40,70,100]), aa_interp);
    }

    // Axis labels
    draw_text(img, "LAT",  (cx + radius + 4) as u32, (cy - 4) as u32,            1, GREY);
    draw_text(img, "LONG", (cx - 8) as u32,           (cy - radius - 14) as u32,  1, GREY);

    // ── History trail — older samples fade out ──
    for (idx, &(hx, hy)) in history.iter().enumerate() {
        let dx = cx + (hx.clamp(-1.0, 1.0) as f64 * radius as f64) as i32;
        let dy = cy - (hy.clamp(-1.0, 1.0) as f64 * radius as f64) as i32;
        let alpha = (25 + idx * 6).min(160) as u8;
        let mag   = (hx * hx + hy * hy).sqrt() as f64;
        let hc    = gforce_color(mag);
        bput(img, dx as u32, dy as u32, Rgba([hc[0],hc[1],hc[2],alpha]));
        // tiny glow
        for off in [-1i32, 0, 1] {
            bput(img, (dx+off) as u32, dy as u32, Rgba([hc[0],hc[1],hc[2],alpha/4]));
            bput(img, dx as u32, (dy+off) as u32, Rgba([hc[0],hc[1],hc[2],alpha/4]));
        }
    }

    // ── Current G vector ──
    let gx = cx + (accel.x.clamp(-1.0, 1.0) as f64 * radius as f64) as i32;
    let gy = cy - (accel.y.clamp(-1.0, 1.0) as f64 * radius as f64) as i32;
    let g_mag = ((accel.x * accel.x + accel.y * accel.y) as f64).sqrt();
    let dc = gforce_color(g_mag.clamp(0.0, 1.5));

    // Tail
    draw_antialiased_line_segment_mut(img, (cx,cy),(gx,gy), Rgba([dc[0],dc[1],dc[2],180]), aa_interp);

    // Glowing dot
    glow_dot(img, gx, gy, 8, dc);
    draw_filled_circle_mut(img, (gx,gy), 3, WHITE);

    // Center cross
    draw_hollow_circle_mut(img, (cx,cy), 5, GREY);
    draw_filled_circle_mut(img, (cx,cy), 2, GREY);

    // G magnitude readout
    let g_str = format!("{:.2}G", g_mag);
    draw_text_c(img, &g_str, cx as u32, (cy + radius + 8) as u32, 2, dc);

    // ── Z-axis bar ──
    let bx = px + 14;
    let by = py + ph - 20;
    let bw = pw - 28;
    let z_n  = ((accel.z.clamp(-1.0, 1.0) + 1.0) / 2.0) as f64;
    let z_fw = (z_n * bw as f64) as u32;
    fill_rect(img, bx, by, bw, 8, Rgba([10, 16, 34, 200]));
    if z_fw > 0 { fill_rect(img, bx, by, z_fw, 8, NEON_CYAN); }
    draw_text(img, "Z", bx + bw + 5, by, 1, GREY);
    corner_bracket(img, bx.saturating_sub(3), by.saturating_sub(3), bw+6, 14, 5, NEON_CYAN_DIM);
}

fn gforce_color(mag: f64) -> Rgba<u8> {
    if      mag < 0.3 { NEON_GREEN }
    else if mag < 0.6 { NEON_YELLOW }
    else if mag < 1.0 { NEON_ORANGE }
    else              { NEON_RED }
}

// ═══════════════════════════════════════════════════════════════════════════════
// COMPASS BAR  (top-center)
// ═══════════════════════════════════════════════════════════════════════════════

pub fn draw_compass(img: &mut RgbaImage, heading: f64, speed: f64, timestamp: &str) {
    let wi = img.width();
    let bw = 720u32;
    let bh = 72u32;
    let bx = wi / 2 - bw / 2;
    let by = 24u32;

    draw_panel(img, bx, by, bw, bh, "");

    // Dim gradient vignette on left/right edges
    for dx in 0..60u32 {
        let a = (60 - dx) as u8 * 2;
        for dy in 0..bh {
            bput(img, bx + dx,        by + dy, Rgba([4, 10, 24, a]));
            bput(img, bx + bw - 1 - dx, by + dy, Rgba([4, 10, 24, a]));
        }
    }

    let deg_visible = 70.0f64;
    let px_per_deg  = bw as f64 / deg_visible;
    let center_x    = (bx + bw / 2) as f64;

    for deg in -80i32..=80 {
        let abs_deg = ((heading as i32 + deg).rem_euclid(360)) as u32;
        let px = center_x + deg as f64 * px_per_deg;
        if px < (bx + 4) as f64 || px >= (bx + bw - 4) as f64 { continue; }
        let pxi = px as u32;

        let is_cardinal = abs_deg % 45 == 0;
        let is_major    = abs_deg % 10 == 0;
        let is_minor    = abs_deg % 5  == 0;

        let tick_h = if is_cardinal { 18u32 } else if is_major { 10 } else if is_minor { 6 } else { 3 };
        let tick_y = by + bh - tick_h - 4;
        let tc = if is_cardinal { NEON_CYAN } else if is_major { WHITE } else { GREY };

        // Draw tick with a thin glow
        for dy in 0..tick_h {
            bput(img, pxi, tick_y + dy, tc);
        }
        if is_cardinal || is_major {
            bput(img, pxi.saturating_sub(1), tick_y, Rgba([tc[0], tc[1], tc[2], 60]));
            bput(img, pxi + 1,               tick_y, Rgba([tc[0], tc[1], tc[2], 60]));
        }

        if is_cardinal {
            let label = match abs_deg {
                0   => "N",  45  => "NE", 90  => "E",  135 => "SE",
                180 => "S",  225 => "SW", 270 => "W",  315 => "NW",
                _   => "",
            };
            if !label.is_empty() {
                let lc = if label == "N" { NEON_ORANGE } else if label == "S" { NEON_RED } else { WHITE };
                draw_text_c(img, label, pxi, by + 6, 2, lc);
            }
        } else if is_major {
            let lbl = format!("{:03}", abs_deg);
            draw_text_c(img, &lbl, pxi, by + bh - 24, 1, Rgba([55, 75, 100, 170]));
        }
    }

    // ── Centre indicator triangle ──
    let tri_x = (bx + bw / 2) as u32;
    let tri_y = by + bh - 3;
    for i in 0..12u32 {
        for j in 0..i {
            bput(img, tri_x.saturating_sub(j), tri_y - i, NEON_CYAN);
            bput(img, tri_x + j,               tri_y - i, NEON_CYAN);
        }
    }
    // glow on triangle
    for i in 0..12u32 {
        for j in 0..i {
            bput(img, tri_x.saturating_sub(j+1), tri_y - i, NEON_CYAN_GLOW);
            bput(img, tri_x + j + 1,             tri_y - i, NEON_CYAN_GLOW);
        }
    }

    // ── Heading badge (dark pill + big text) ──
    let hdg_str = format!("{:03.0}", heading);
    let hw = tw(&hdg_str, 3) + 14;
    let hh = th(3) + 8;
    let hx = tri_x.saturating_sub(hw / 2);
    let hy = by + 5;
    fill_rect(img, hx, hy, hw, hh, Rgba([0, 18, 38, 220]));
    corner_bracket(img, hx, hy, hw, hh, 6, NEON_CYAN);
    draw_text_c(img, &hdg_str, tri_x, hy + 4, 3, NEON_CYAN);

    // ── Speed readout — left ──
    let spd_str = format!("{:3.0}", speed);
    draw_text(img, &spd_str, bx + 12, by + bh / 2 - 10, 2, WHITE);
    draw_text(img, "km/h",   bx + 12, by + bh / 2 + 6,  1, GREY);

    // ── Timestamp — right ──
    let ts = if timestamp.len() > 8 { &timestamp[timestamp.len()-8..] } else { timestamp };
    if !ts.is_empty() {
        let tsw = tw(ts, 1);
        draw_text(img, ts, bx + bw - tsw - 12, by + bh / 2 - 4, 1, GREY);
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// GPS TAG  (top-right)
// ═══════════════════════════════════════════════════════════════════════════════

pub fn draw_gps_tag(img: &mut RgbaImage, lat: f64, lon: f64) {
    let wi    = img.width();
    let tw_   = 310u32;
    let th_   = 72u32;
    let margin = 26u32;
    let tx = wi - tw_ - margin;
    let ty = 24u32;

    draw_panel(img, tx, ty, tw_, th_, "GPS");

    let lat_str = format!("{:.6}", lat);
    let lon_str = format!("{:.6}", lon);

    // Labels in dim cyan, values in white
    draw_text(img, "LAT", tx + 8, ty + 13, 1, NEON_CYAN_DIM);
    draw_text(img, &lat_str, tx + 38, ty + 11, 2, WHITE);

    draw_text(img, "LON", tx + 8, ty + 38, 1, NEON_CYAN_DIM);
    draw_text(img, &lon_str, tx + 38, ty + 36, 2, WHITE);
}

// ═══════════════════════════════════════════════════════════════════════════════
// MINI-MAP
// ═══════════════════════════════════════════════════════════════════════════════

/// Pre-baked bounding box cache — computed once, reused every frame.
/// Avoids scanning all telemetry on every render call.
pub struct MinimapCache {
    pub lat_min: f64,
    pub lat_max: f64,
    pub lon_min: f64,
    pub lon_max: f64,
    pub size:    u32,
    pub margin:  u32,
}

impl MinimapCache {
    pub fn build(all_frames: &[TelemetryFrame], size: u32, margin: u32) -> Self {
        let lat_min = all_frames.iter().filter_map(|f| f.gps.as_ref().map(|g| g.latitude)).fold(f64::INFINITY,     f64::min);
        let lat_max = all_frames.iter().filter_map(|f| f.gps.as_ref().map(|g| g.latitude)).fold(f64::NEG_INFINITY, f64::max);
        let lon_min = all_frames.iter().filter_map(|f| f.gps.as_ref().map(|g| g.longitude)).fold(f64::INFINITY,    f64::min);
        let lon_max = all_frames.iter().filter_map(|f| f.gps.as_ref().map(|g| g.longitude)).fold(f64::NEG_INFINITY,f64::max);
        Self { lat_min, lat_max, lon_min, lon_max, size, margin }
    }
}

pub fn draw_minimap(
    img: &mut RgbaImage,
    all_frames: &[TelemetryFrame],
    cache: &MinimapCache,
) {
    if all_frames.len() < 2 { return; }

    let size   = cache.size;
    let margin = cache.margin;
    let lat_range = (cache.lat_max - cache.lat_min).max(1e-9);
    let lon_range = (cache.lon_max - cache.lon_min).max(1e-9);
    let ox = img.width()  - size - margin;
    let oy = img.height() - size - margin;

    draw_panel(img, ox, oy, size, size, "MAP");

    let pad = 20u32;
    let inner = size - pad * 2;

    let to_px = |lon: f64, lat: f64| -> (i32, i32) {
        let x = ox + pad + ((lon - cache.lon_min) / lon_range * inner as f64) as u32;
        let y = oy + pad + ((cache.lat_max - lat) / lat_range * inner as f64) as u32;
        (x as i32, y as i32)
    };

    // Full route (very dim — static, pre-drawn in frame 0)
    for win in all_frames.windows(2) {
        if let (Some(ga), Some(gb)) = (&win[0].gps, &win[1].gps) {
            let (x0, y0) = to_px(ga.longitude, ga.latitude);
            let (x1, y1) = to_px(gb.longitude, gb.latitude);
            draw_antialiased_line_segment_mut(img, (x0,y0),(x1,y1), Rgba([25,44,75,130]), aa_interp);
        }
    }

    // Recent trail — last 50 telemetry steps, fading cyan
    let trail: Vec<_> = all_frames.iter().rev().take(50).collect();
    for (idx, win) in trail.windows(2).enumerate() {
        let (a, b) = (win[1], win[0]);
        if let (Some(ga), Some(gb)) = (&a.gps, &b.gps) {
            let (x0, y0) = to_px(ga.longitude, ga.latitude);
            let (x1, y1) = to_px(gb.longitude, gb.latitude);
            let alpha = (220u32.saturating_sub(idx as u32 * 4)).max(35) as u8;
            draw_antialiased_line_segment_mut(img, (x0,y0),(x1,y1), Rgba([0,200,255,alpha]), aa_interp);
            let ga2 = alpha / 5;
            for off in [-1i32, 1] {
                draw_antialiased_line_segment_mut(img, (x0+off,y0),(x1+off,y1), Rgba([0,200,255,ga2]), aa_interp);
                draw_antialiased_line_segment_mut(img, (x0,y0+off),(x1,y1+off), Rgba([0,200,255,ga2]), aa_interp);
            }
        }
    }

    // Current position
    if let Some(gps) = all_frames.last().and_then(|f| f.gps.as_ref()) {
        let (dx, dy) = to_px(gps.longitude, gps.latitude);
        glow_dot(img, dx, dy, 6, NEON_CYAN);
        draw_filled_circle_mut(img, (dx, dy), 2, WHITE);
        if let Some(h) = gps.heading {
            let angle = h.to_radians() - PI / 2.0;
            let ax = (dx as f64 + 16.0 * angle.cos()) as i32;
            let ay = (dy as f64 + 16.0 * angle.sin()) as i32;
            glow_line(img, dx, dy, ax, ay, NEON_YELLOW, 2);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// FULL HUD COMPOSITOR
// ═══════════════════════════════════════════════════════════════════════════════

pub fn draw_hud(
    img: &mut RgbaImage,
    frame: &TelemetryFrame,
    all_frames: &[TelemetryFrame],
    max_speed: f64,
    g_history: &[(f32, f32)],
    minimap_cache: &MinimapCache,
    elements: HudElements,
) {
    let h = img.height() as i32;

    if elements.speedometer {
        let r  = 195i32;
        let cx = r + 36;
        let cy = h - r - 36;
        if let Some(gps) = &frame.gps {
            draw_speedometer(img, gps.speed.unwrap_or(0.0), max_speed, cx, cy, r);
        }
    }

    if elements.gforce {
        let r  = 130i32;
        let cx = r + 36;
        let cy = r + 36;
        if let Some(accel) = &frame.accel {
            draw_gforce(img, accel, cx, cy, r, g_history);
        }
    }

    if elements.compass {
        if let Some(gps) = &frame.gps {
            draw_compass(img, gps.heading.unwrap_or(0.0), gps.speed.unwrap_or(0.0), &gps.timestamp);
        }
    }

    if elements.gps_tag {
        if let Some(gps) = &frame.gps {
            draw_gps_tag(img, gps.latitude, gps.longitude);
        }
    }

    if elements.minimap {
        draw_minimap(img, all_frames, minimap_cache);
    }
}