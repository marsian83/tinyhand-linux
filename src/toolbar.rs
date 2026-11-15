//! The toolbar shown at the bottom of the screen while the pointer is on (port of
//! Toolbar.swift): pinned hands, a divider, a sound toggle and an exit button on a dark
//! rounded bar. Icons only, no text. Everything is drawn here in software with
//! anti-aliased signed-distance shapes into a premultiplied 0xAARRGGBB buffer.
//!
//! Coordinates: "logical" px (what `size`, `hover` and `click` use) times `scale` gives
//! device px (what `buffer_size` and `render` use). The surface includes a margin around
//! the bar for its drop shadow; points in that margin hit nothing.

use crate::{config, hand, theme::Theme};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Hand(String),
    ToggleSound,
    Exit,
}

// Layout, in logical px (same numbers as the macOS stack view).
const BTN: f32 = 40.0;
const GAP: f32 = 6.0;
const PAD_X: f32 = 10.0;
const PAD_Y: f32 = 8.0;
const BAR_H: f32 = BTN + 2.0 * PAD_Y;
const RADIUS: f32 = 16.0;
const BTN_RADIUS: f32 = 10.0;
const DIV_W: f32 = 1.0;
const DIV_H: f32 = 24.0;
/// Hand thumbnails are drawn this big inside their 40 px button.
const THUMB: f32 = 36.0;
// Shadow margin around the bar. Surface w/h are rounded up to multiples of 20 so the
// device size is exact at scales 1, 1.25, 1.5, 1.6 and 2.
const M_TOP: f32 = 8.0;
const M_BOTTOM: f32 = 16.0;
const M_SIDE: f32 = 12.0;
const SNAP: i32 = 20;

#[derive(Clone, Debug, PartialEq)]
enum Kind {
    Hand(String),
    Sound,
    Exit,
}

struct Button {
    kind: Kind,
    /// Logical x of the button's left edge (top edge is `M_TOP + PAD_Y`).
    x: f32,
}

/// A hand thumbnail at its final device size, premultiplied 0xAARRGGBB.
struct Thumb {
    side: usize,
    px: Vec<u32>,
}

pub struct Toolbar {
    pinned: Vec<String>,
    current: String,
    sound_on: bool,
    scale: f32,
    buttons: Vec<Button>,
    divider_x: f32,
    bar_x: f32,
    bar_w: f32,
    w: i32,
    h: i32,
    hover: Option<usize>,
    pal: Palette,
    /// Decoded lazily on first render; `None` = failed (a coloured dot is drawn instead).
    thumbs: HashMap<String, Option<Thumb>>,
}

impl Toolbar {
    /// `pinned`: hand ids ("laser" allowed). `current`: current hand id. `scale`: device px per logical px.
    /// `theme`: the desktop's light/dark preference and accent colour.
    /// Thumbnails are decoded lazily on first render.
    pub fn new(pinned: &[&str], current: &str, sound_on: bool, scale: f32, theme: Theme) -> Toolbar {
        let mut t = Toolbar {
            pinned: pinned.iter().map(|s| s.to_string()).collect(),
            current: current.to_string(),
            sound_on,
            scale: if scale > 0.0 { scale } else { 1.0 },
            buttons: Vec::new(),
            divider_x: 0.0,
            bar_x: 0.0,
            bar_w: 0.0,
            w: 0,
            h: 0,
            hover: None,
            pal: Palette::new(theme),
            thumbs: HashMap::new(),
        };
        t.layout();
        t
    }

    /// Logical size (w, h) of the surface, including the shadow margin.
    pub fn size(&self) -> (i32, i32) {
        (self.w, self.h)
    }

    /// Buffer size in device px.
    pub fn buffer_size(&self) -> (usize, usize) {
        ((self.w as f32 * self.scale).round() as usize, (self.h as f32 * self.scale).round() as usize)
    }

    /// The bar itself (without the shadow margin) in logical px: (x, y, w, h). Handy for
    /// an input region so clicks on the shadow margin fall through.
    pub fn body_rect(&self) -> (i32, i32, i32, i32) {
        (self.bar_x as i32, M_TOP as i32, self.bar_w as i32, BAR_H as i32)
    }

    /// Render premultiplied ARGB, row-major, fully overwriting `out` (buffer_size() pixels).
    pub fn render(&mut self, out: &mut [u32]) {
        let (bw, bh) = self.buffer_size();
        assert!(out.len() >= bw * bh, "toolbar buffer too small");
        let out = &mut out[..bw * bh];
        self.load_thumbs();
        self.draw_bar(out, bw, bh, true);
        let pal = self.pal;
        let mut c = Canvas { px: out, w: bw, h: bh, s: self.scale };

        // Divider.
        let dy = M_TOP + BAR_H / 2.0;
        let dx = self.divider_x + DIV_W / 2.0;
        c.fill(pal.fg(0.18), (dx - 1.0, dy - DIV_H / 2.0 - 1.0, dx + 1.0, dy + DIV_H / 2.0 + 1.0), |x, y| {
            sd_box(x, y, dx, dy, DIV_W / 2.0, DIV_H / 2.0)
        });

        let by = M_TOP + PAD_Y;
        for (i, b) in self.buttons.iter().enumerate() {
            let selected = matches!(&b.kind, Kind::Hand(id) if *id == self.current);
            let hovered = self.hover == Some(i);
            let bg = match (selected, hovered) {
                (true, true) => Some(pal.selected(true)),
                (true, false) => Some(pal.selected(false)),
                (false, true) => Some(pal.fg(0.09)),
                _ => None,
            };
            let (cx, cy) = (b.x + BTN / 2.0, by + BTN / 2.0);
            if let Some(bg) = bg {
                c.fill(bg, (b.x, by, b.x + BTN, by + BTN), |x, y| {
                    sd_rrect(x, y, cx, cy, BTN / 2.0, BTN / 2.0, BTN_RADIUS)
                });
            }
            match &b.kind {
                Kind::Hand(id) if id == config::LASER_ID => draw_laser(&mut c, cx, cy),
                Kind::Hand(id) => match self.thumbs.get(id) {
                    Some(Some(t)) => c.blit(t, cx, cy),
                    _ => draw_fallback(&mut c, cx, cy, id),
                },
                Kind::Sound => draw_speaker(&mut c, cx, cy, self.sound_on, pal),
                Kind::Exit => draw_xmark(&mut c, cx, cy, pal),
            }
        }
    }

    /// Hover feedback; returns true if the look changed (caller re-renders).
    pub fn hover(&mut self, p: Option<(f32, f32)>) -> bool {
        let h = p.and_then(|p| self.hit(p));
        let changed = h != self.hover;
        self.hover = h;
        changed
    }

    /// Hit test in the bar's logical coordinates (origin top-left of the surface).
    pub fn click(&self, p: (f32, f32)) -> Option<Action> {
        self.hit(p).map(|i| match &self.buttons[i].kind {
            Kind::Hand(id) => Action::Hand(id.clone()),
            Kind::Sound => Action::ToggleSound,
            Kind::Exit => Action::Exit,
        })
    }

    /// Select a hand. Like macOS, a hand that isn't pinned gets its own button at the end
    /// of the hands, so `size()` can change: callers should re-check it afterwards.
    pub fn set_current(&mut self, hand: &str) {
        if hand != self.current {
            self.current = hand.to_string();
            self.layout();
        }
    }

    pub fn set_sound(&mut self, on: bool) {
        self.sound_on = on;
    }

    fn hit(&self, (x, y): (f32, f32)) -> Option<usize> {
        let by = M_TOP + PAD_Y;
        if !(by..by + BTN).contains(&y) {
            return None;
        }
        self.buttons.iter().position(|b| (b.x..b.x + BTN).contains(&x))
    }

    fn layout(&mut self) {
        let mut hands: Vec<String> = self.pinned.clone();
        if !self.current.is_empty() && !hands.contains(&self.current) {
            hands.push(self.current.clone());
        }
        // Content: hands, divider, sound, exit; GAP between consecutive items.
        let n_items = hands.len() + 3;
        let content = hands.len() as f32 * BTN + DIV_W + 2.0 * BTN + GAP * (n_items - 1) as f32;
        let bar_w = content + 2.0 * PAD_X;
        let w = round_up(bar_w + 2.0 * M_SIDE, SNAP);
        let h = round_up(M_TOP + BAR_H + M_BOTTOM, SNAP);
        // Integral bar position; if the spare width is odd the bar grows by 1 px instead.
        let bar_x = ((w as f32 - bar_w) / 2.0).floor();
        self.bar_w = w as f32 - 2.0 * bar_x;
        self.bar_x = bar_x;
        self.w = w;
        self.h = h;
        let mut x = bar_x + PAD_X + (self.bar_w - bar_w) / 2.0;
        self.buttons.clear();
        for id in hands {
            self.buttons.push(Button { kind: Kind::Hand(id), x });
            x += BTN + GAP;
        }
        self.divider_x = x;
        x += DIV_W + GAP;
        self.buttons.push(Button { kind: Kind::Sound, x });
        x += BTN + GAP;
        self.buttons.push(Button { kind: Kind::Exit, x });
        self.hover = None;
        // Only keep thumbnails that are on the bar.
        let shown: Vec<&str> = self
            .buttons
            .iter()
            .filter_map(|b| if let Kind::Hand(id) = &b.kind { Some(id.as_str()) } else { None })
            .collect();
        self.thumbs.retain(|k, _| shown.contains(&k.as_str()));
    }

    fn load_thumbs(&mut self) {
        let side = (THUMB * self.scale).round().max(1.0) as usize;
        for b in &self.buttons {
            if let Kind::Hand(id) = &b.kind {
                if id != config::LASER_ID && !self.thumbs.contains_key(id) {
                    self.thumbs.insert(id.clone(), make_thumb(id, side));
                }
            }
        }
    }

    /// Shadow, bar body and its 1 px inner border, overwriting every pixel.
    fn draw_bar(&self, out: &mut [u32], bw: usize, bh: usize, fast: bool) {
        let s = self.scale;
        let (cx, cy) = (self.bar_x + self.bar_w / 2.0, M_TOP + BAR_H / 2.0);
        let (hw, hh) = (self.bar_w / 2.0, BAR_H / 2.0);
        let border = 1.0 * s; // device px
                              // Two shadows: a wide soft one and a tight contact one (logical px).
        const S1: (f32, f32, f32) = (3.0, 5.0, 0.38); // (y offset, sigma, alpha)
        const S2: (f32, f32, f32) = (0.5, 1.2, 0.22);
        let top = M_TOP;
        let symmetric = fast && (self.w as f32 * s - bw as f32).abs() < 1e-3;
        let xe = (((self.bar_x + RADIUS + 1.0) * s).ceil().max(0.0) as usize).min(bw / 2);
        for y in 0..bh {
            let ly = (y as f32 + 0.5) / s;
            // Body colour with a faint vertical sheen, lighter at the top.
            let t = ((ly - top) / BAR_H).clamp(0.0, 1.0);
            let body = self.pal.body(t);
            let interior = pack(body);
            let edge = self.pal.fg(self.pal.edge * (1.0 - t / 3.0));
            let at = |x: usize| -> u32 {
                let lx = (x as f32 + 0.5) / s;
                let d = sd_rrect(lx, ly, cx, cy, hw, hh, RADIUS) * s;
                if d < -border - 1.0 {
                    return interior;
                }
                // Shadow (normal CDF of the distance, logistic approximation).
                let sh = |o: (f32, f32, f32)| {
                    let ds = sd_rrect(lx, ly - o.0, cx, cy, hw, hh, RADIUS);
                    o.2 / (1.0 + (1.702 * ds / o.1).exp())
                };
                let a = 1.0 - (1.0 - sh(S1)) * (1.0 - sh(S2));
                let mut c = [a, 0.0, 0.0, 0.0];
                let cov = (0.5 - d).clamp(0.0, 1.0);
                if cov > 0.0 {
                    c = over(c, body, cov);
                    let inner = (0.5 - (d + border)).clamp(0.0, 1.0);
                    c = over(c, edge, cov - inner);
                }
                pack(c)
            };
            let row = &mut out[y * bw..(y + 1) * bw];
            if !symmetric {
                for (x, px) in row.iter_mut().enumerate() {
                    *px = at(x);
                }
                continue;
            }
            // The bar is symmetric about the surface centre, and between the corners
            // each row is a single value (shadows are offset vertically only).
            for x in 0..xe {
                let v = at(x);
                row[x] = v;
                row[bw - 1 - x] = v;
            }
            let mid = at(xe);
            row[xe..bw - xe].fill(mid);
        }
    }
}

fn round_up(v: f32, m: i32) -> i32 {
    let v = v.ceil() as i32;
    (v + m - 1) / m * m
}

// ---------------------------------------------------------------------------------------
// Pixels: premultiplied [a, r, g, b] in 0..1 while blending.

/// The bar's colours, from the desktop theme.
#[derive(Clone, Copy)]
struct Palette {
    /// Foreground (icons, divider, hover) as straight 0..1 RGB.
    fg: [f32; 3],
    /// Body RGB at the top and bottom of the bar, 0..255, and its opacity.
    body_top: [f32; 3],
    body_bottom: [f32; 3],
    body_alpha: f32,
    /// Opacity of the 1 px inner border at the top of the bar.
    edge: f32,
    /// The selected hand's button: the accent colour, or the foreground.
    accent: Option<[f32; 3]>,
}

impl Palette {
    fn new(theme: Theme) -> Palette {
        if theme.dark {
            Palette {
                fg: [1.0, 1.0, 1.0],
                body_top: [32.0, 32.0, 35.0],
                body_bottom: [25.0, 25.0, 28.0],
                body_alpha: 0.92,
                edge: 0.12,
                accent: theme.accent,
            }
        } else {
            Palette {
                fg: [0.0, 0.0, 6.0 / 255.0],
                body_top: [252.0, 252.0, 253.0],
                body_bottom: [240.0, 240.0, 243.0],
                body_alpha: 0.94,
                edge: 0.10,
                accent: theme.accent,
            }
        }
    }

    /// Premultiplied foreground at opacity `a`.
    fn fg(&self, a: f32) -> [f32; 4] {
        premul(self.fg, a)
    }

    /// Premultiplied body colour `t` of the way down the bar (0 = top, 1 = bottom).
    fn body(&self, t: f32) -> [f32; 4] {
        let c = [0, 1, 2].map(|i| (self.body_top[i] + (self.body_bottom[i] - self.body_top[i]) * t) / 255.0);
        premul(c, self.body_alpha)
    }

    /// The selected hand's button background.
    fn selected(&self, hovered: bool) -> [f32; 4] {
        match self.accent {
            Some(c) => premul(c, if hovered { 0.55 } else { 0.45 }),
            None => self.fg(if hovered { 0.24 } else { 0.18 }),
        }
    }
}

fn premul(c: [f32; 3], a: f32) -> [f32; 4] {
    [a, c[0] * a, c[1] * a, c[2] * a]
}

fn rgb(c: u32, a: f32) -> [f32; 4] {
    let ch = |sh: u32| ((c >> sh) & 0xFF) as f32 / 255.0 * a;
    [a, ch(16), ch(8), ch(0)]
}

#[inline]
fn over(dst: [f32; 4], src: [f32; 4], cov: f32) -> [f32; 4] {
    let k = 1.0 - src[0] * cov;
    [src[0] * cov + dst[0] * k, src[1] * cov + dst[1] * k, src[2] * cov + dst[2] * k, src[3] * cov + dst[3] * k]
}

#[inline]
fn pack(c: [f32; 4]) -> u32 {
    let q = |v: f32| (v * 255.0 + 0.5).clamp(0.0, 255.0) as u32;
    let a = q(c[0]);
    a << 24 | q(c[1]).min(a) << 16 | q(c[2]).min(a) << 8 | q(c[3]).min(a)
}

#[inline]
fn unpack(p: u32) -> [f32; 4] {
    let ch = |sh: u32| ((p >> sh) & 0xFF) as f32 / 255.0;
    [ch(24), ch(16), ch(8), ch(0)]
}

/// The output buffer plus the scale, for drawing shapes given in logical px.
struct Canvas<'a> {
    px: &'a mut [u32],
    w: usize,
    h: usize,
    s: f32,
}

impl Canvas<'_> {
    /// Fill a shape: `sd` gives the signed distance (logical px) at a logical point; only
    /// pixels inside `bounds` (logical x0, y0, x1, y1) are visited.
    fn fill(&mut self, color: [f32; 4], bounds: (f32, f32, f32, f32), sd: impl Fn(f32, f32) -> f32) {
        let s = self.s;
        self.paint(bounds, |x, y| {
            let cov = (0.5 - sd(x, y) * s).clamp(0.0, 1.0);
            (color, cov)
        });
    }

    /// Generic: `f` returns (colour, coverage) at a logical point.
    fn paint(&mut self, (x0, y0, x1, y1): (f32, f32, f32, f32), f: impl Fn(f32, f32) -> ([f32; 4], f32)) {
        let s = self.s;
        let xa = ((x0 * s).floor().max(0.0) as usize).min(self.w);
        let xb = ((x1 * s).ceil().max(0.0) as usize).min(self.w);
        let ya = ((y0 * s).floor().max(0.0) as usize).min(self.h);
        let yb = ((y1 * s).ceil().max(0.0) as usize).min(self.h);
        for y in ya..yb {
            let ly = (y as f32 + 0.5) / s;
            for x in xa..xb {
                let (c, cov) = f((x as f32 + 0.5) / s, ly);
                if cov <= 0.0 || c[0] <= 0.0 {
                    continue;
                }
                let p = &mut self.px[y * self.w + x];
                *p = pack(over(unpack(*p), c, cov));
            }
        }
    }

    /// Composite a thumbnail centred on a logical point, aligned to device pixels.
    fn blit(&mut self, t: &Thumb, cx: f32, cy: f32) {
        let x0 = (cx * self.s - t.side as f32 / 2.0).round() as isize;
        let y0 = (cy * self.s - t.side as f32 / 2.0).round() as isize;
        for ty in 0..t.side {
            let y = y0 + ty as isize;
            if y < 0 || y >= self.h as isize {
                continue;
            }
            for tx in 0..t.side {
                let x = x0 + tx as isize;
                if x < 0 || x >= self.w as isize {
                    continue;
                }
                let src = t.px[ty * t.side + tx];
                let sa = src >> 24;
                if sa == 0 {
                    continue;
                }
                let d = &mut self.px[y as usize * self.w + x as usize];
                if sa == 255 {
                    *d = src;
                    continue;
                }
                // Integer premultiplied src-over per channel.
                let k = 255 - sa;
                let ch = |sh: u32| {
                    let v = ((src >> sh) & 0xFF) + (((*d >> sh) & 0xFF) * k + 127) / 255;
                    v.min(255) << sh
                };
                *d = ch(24) | ch(16) | ch(8) | ch(0);
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// Signed distances, logical px; negative inside.

#[inline]
fn sd_rrect(px: f32, py: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let qx = (px - cx).abs() - hw + r;
    let qy = (py - cy).abs() - hh + r;
    let (ox, oy) = (qx.max(0.0), qy.max(0.0));
    (ox * ox + oy * oy).sqrt() + qx.max(qy).min(0.0) - r
}

#[inline]
fn sd_box(px: f32, py: f32, cx: f32, cy: f32, hw: f32, hh: f32) -> f32 {
    sd_rrect(px, py, cx, cy, hw, hh, 0.0)
}

#[inline]
fn dist_segment(px: f32, py: f32, (ax, ay): (f32, f32), (bx, by): (f32, f32)) -> f32 {
    let (pax, pay, bax, bay) = (px - ax, py - ay, bx - ax, by - ay);
    let h = ((pax * bax + pay * bay) / (bax * bax + bay * bay)).clamp(0.0, 1.0);
    let (dx, dy) = (pax - bax * h, pay - bay * h);
    (dx * dx + dy * dy).sqrt()
}

/// Distance to an arc of radius `r` around (cx, cy), opening to the right, spanning
/// ±`half` radians (end points count as the arc's ends).
#[inline]
fn dist_arc(px: f32, py: f32, cx: f32, cy: f32, r: f32, half: f32) -> f32 {
    let (x, y) = (px - cx, (py - cy).abs());
    if y.atan2(x) <= half {
        ((x * x + y * y).sqrt() - r).abs()
    } else {
        let (ex, ey) = (r * half.cos(), r * half.sin());
        ((x - ex) * (x - ex) + (y - ey) * (y - ey)).sqrt()
    }
}

/// Signed distance to a polygon (Inigo Quilez's sdPolygon).
fn sd_polygon(px: f32, py: f32, v: &[(f32, f32)]) -> f32 {
    let mut d = (px - v[0].0).powi(2) + (py - v[0].1).powi(2);
    let mut sign = 1.0;
    let mut j = v.len() - 1;
    for i in 0..v.len() {
        let (ex, ey) = (v[j].0 - v[i].0, v[j].1 - v[i].1);
        let (wx, wy) = (px - v[i].0, py - v[i].1);
        let h = ((wx * ex + wy * ey) / (ex * ex + ey * ey)).clamp(0.0, 1.0);
        let (bx, by) = (wx - ex * h, wy - ey * h);
        d = d.min(bx * bx + by * by);
        let c = [py >= v[i].1, py < v[j].1, ex * wy > ey * wx];
        if c.iter().all(|&b| b) || c.iter().all(|&b| !b) {
            sign = -sign;
        }
        j = i;
    }
    sign * d.sqrt()
}

// ---------------------------------------------------------------------------------------
// Icons. All centred on (cx, cy), sized like the macOS 15 pt SF Symbols.

fn cov(d: f32, s: f32) -> f32 {
    (0.5 - d * s).clamp(0.0, 1.0)
}

/// Filled speaker (rounded back plate + flared cone), distance relative to its own origin.
fn speaker_sd(x: f32, y: f32, ox: f32, oy: f32) -> f32 {
    let (x, y) = (x - ox, y - oy);
    let round = 0.9;
    let back = sd_rrect(x, y, -5.6, 0.0, 2.6, 3.4, 1.3);
    let cone = sd_polygon(
        x,
        y,
        &[
            (-4.6, -3.4 + round),
            (0.3 - round, -7.6 + round * 1.6),
            (0.3 - round, 7.6 - round * 1.6),
            (-4.6, 3.4 - round),
        ],
    ) - round;
    back.min(cone)
}

fn draw_speaker(c: &mut Canvas, cx: f32, cy: f32, on: bool, pal: Palette) {
    let s = c.s;
    let color = pal.fg(if on { 0.85 } else { 0.45 });
    let b = (cx - 12.0, cy - 11.0, cx + 12.0, cy + 11.0);
    if on {
        // Speaker on the left, two waves on the right; glyph is ~17 px wide.
        let ox = cx + 0.0;
        let stroke = 0.95;
        c.paint(b, |x, y| {
            let body = cov(speaker_sd(x, y, ox, cy), s);
            let w1 = cov(dist_arc(x, y, ox + 0.3, cy, 4.0, 0.85) - stroke, s);
            let w2 = cov(dist_arc(x, y, ox + 0.3, cy, 7.6, 0.90) - stroke, s);
            (color, body.max(w1).max(w2))
        });
    } else {
        // speaker.slash.fill: the speaker cut by a diagonal stroke.
        let ox = cx + 2.6;
        let (a, e) = ((cx - 6.5, cy - 7.5), (cx + 6.5, cy + 7.5));
        c.paint(b, |x, y| {
            let body = cov(speaker_sd(x, y, ox, cy), s);
            let dl = dist_segment(x, y, a, e);
            let gap = cov(dl - 2.2, s);
            let line = cov(dl - 0.95, s);
            (color, (body * (1.0 - gap)).max(line))
        });
    }
}

fn draw_xmark(c: &mut Canvas, cx: f32, cy: f32, pal: Palette) {
    let r = 5.2;
    let (a, b, p, q) = ((cx - r, cy - r), (cx + r, cy + r), (cx - r, cy + r), (cx + r, cy - r));
    c.fill(pal.fg(0.85), (cx - 8.0, cy - 8.0, cx + 8.0, cy + 8.0), |x, y| {
        dist_segment(x, y, a, b).min(dist_segment(x, y, p, q)) - 1.0
    });
}

/// The laser icon: a red dot with a soft glow and a pale core (Toolbar.icon in Swift,
/// whose 64 pt drawing is shown at 40 px).
fn draw_laser(c: &mut Canvas, cx: f32, cy: f32) {
    let k = BTN / 64.0;
    let glow_r = 18.0 * k;
    let red = rgb(config::LASER, 1.0);
    let b = (cx - glow_r, cy - glow_r, cx + glow_r, cy + glow_r);
    c.paint(b, |x, y| {
        let r = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
        let t = (1.0 - r / glow_r).clamp(0.0, 1.0);
        (red, 0.6 * t)
    });
    c.fill(red, b, |x, y| ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - 7.0 * k);
    let core = [0.95, 0.95, 0.86 * 0.95, 0.82 * 0.95];
    c.fill(core, b, |x, y| ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - 3.0 * k);
}

/// Shown when a hand image can't be loaded: a dot in the hand's ink colour.
fn draw_fallback(c: &mut Canvas, cx: f32, cy: f32, id: &str) {
    let ink = config::hand_ink(id).unwrap_or(0x8E8E93);
    let b = (cx - 12.0, cy - 12.0, cx + 12.0, cy + 12.0);
    c.fill([0.35, 0.0, 0.0, 0.0], b, |x, y| ((x - cx).powi(2) + (y - cy - 1.0).powi(2)).sqrt() - 9.5);
    c.fill(rgb(ink, 1.0), b, |x, y| ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - 9.0);
}

// ---------------------------------------------------------------------------------------
// Thumbnails (HandAsset.thumbnail in PointerStyle.swift): a square around the hand,
// 6% larger than the hand's bigger side, centred on it. Area outside the image is
// transparent rather than stretched.

fn make_thumb(id: &str, side: usize) -> Option<Thumb> {
    let path = hand::resolve(id).ok()?;
    let img = hand::load_png(&path).ok()?;
    let m = hand::measure(&img)?;
    let (w, h) = (img.w as f32, img.h as f32);
    let hand_w = (m.hand_max_x - m.hand_min_x) * w;
    let hand_h = (m.wrist - m.hand_top) * h;
    let crop = hand_w.max(hand_h) * 1.06;
    let cx = (m.hand_min_x + m.hand_max_x) / 2.0 * w;
    let cy = (m.hand_top + m.wrist) / 2.0 * h;
    Some(Thumb { side, px: downscale(&img, cx - crop / 2.0, cy - crop / 2.0, crop, side) })
}

/// Area-average the square (x0, y0, size) of `img` (straight RGBA) into side×side
/// premultiplied 0xAARRGGBB.
fn downscale(img: &hand::Image, x0: f32, y0: f32, size: f32, side: usize) -> Vec<u32> {
    let k = size / side as f32; // source px per output px
                                // Overlap of source cell [i, i+1) with output cell o along one axis.
    let taps = |origin: f32, len: usize| -> Vec<(usize, Vec<f32>)> {
        (0..side)
            .map(|o| {
                let a = origin + o as f32 * k;
                let b = a + k;
                let lo = a.floor().max(0.0) as usize;
                let hi = (b.ceil().max(0.0) as usize).min(len);
                let ws = (lo..hi.max(lo)).map(|i| ((b.min(i as f32 + 1.0) - a.max(i as f32)).max(0.0)) / k).collect();
                (lo, ws)
            })
            .collect()
    };
    let xt = taps(x0, img.w);
    let yt = taps(y0, img.h);
    let mut row = vec![[0f32; 4]; side];
    let mut acc = vec![[0f32; 4]; side * side];
    for (oy, (ylo, yws)) in yt.iter().enumerate() {
        for (j, &wy) in yws.iter().enumerate() {
            if wy <= 0.0 {
                continue;
            }
            let src = &img.rgba[(ylo + j) * img.w * 4..(ylo + j + 1) * img.w * 4];
            for (o, (xlo, xws)) in xt.iter().enumerate() {
                let mut a = [0f32; 4];
                for (i, &wx) in xws.iter().enumerate() {
                    let p = &src[(xlo + i) * 4..(xlo + i) * 4 + 4];
                    if p[3] == 0 {
                        continue;
                    }
                    let al = p[3] as f32 * wx;
                    a[0] += al;
                    a[1] += p[0] as f32 * al;
                    a[2] += p[1] as f32 * al;
                    a[3] += p[2] as f32 * al;
                }
                row[o] = a;
            }
            for (o, r) in row.iter().enumerate() {
                let d = &mut acc[oy * side + o];
                for c in 0..4 {
                    d[c] += r[c] * wy;
                }
            }
        }
    }
    acc.iter()
        .map(|c| {
            let a = c[0].round().clamp(0.0, 255.0) as u32;
            let ch = |v: f32| ((v / 255.0).round().clamp(0.0, 255.0) as u32).min(a);
            a << 24 | ch(c[1]) << 16 | ch(c[2]) << 8 | ch(c[3])
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PINNED: &[&str] = &["lavender", "glove", "coral", "plush", "robot", "laser"];

    fn centre(t: &Toolbar, i: usize) -> (f32, f32) {
        (t.buttons[i].x + BTN / 2.0, M_TOP + PAD_Y + BTN / 2.0)
    }

    #[test]
    fn sizes_are_exact_at_common_scales() {
        for &s in &[1.0f32, 1.25, 1.5, 1.6, 2.0] {
            let t = Toolbar::new(PINNED, "coral", true, s, Theme::FALLBACK);
            let (w, h) = t.size();
            let (bw, bh) = t.buffer_size();
            assert!((w as f32 * s - bw as f32).abs() < 1e-3, "scale {s}");
            assert!((h as f32 * s - bh as f32).abs() < 1e-3, "scale {s}");
            assert_eq!(w % 20, 0);
            assert_eq!(h % 20, 0);
        }
    }

    #[test]
    fn buttons_hit_their_actions() {
        let t = Toolbar::new(PINNED, "coral", true, 1.6, Theme::FALLBACK);
        assert_eq!(t.buttons.len(), PINNED.len() + 2);
        for (i, id) in PINNED.iter().enumerate() {
            assert_eq!(t.click(centre(&t, i)), Some(Action::Hand(id.to_string())));
        }
        assert_eq!(t.click(centre(&t, PINNED.len())), Some(Action::ToggleSound));
        assert_eq!(t.click(centre(&t, PINNED.len() + 1)), Some(Action::Exit));
        // Button corners are inside, the gaps between buttons are not.
        let b = &t.buttons[0];
        let top = M_TOP + PAD_Y;
        assert_eq!(t.click((b.x + 0.1, top + 0.1)), Some(Action::Hand("lavender".into())));
        assert_eq!(t.click((b.x + BTN + GAP / 2.0, top + BTN / 2.0)), None);
        // Divider, bar padding, shadow margin and outside.
        assert_eq!(t.click((t.divider_x + 0.5, top + BTN / 2.0)), None);
        assert_eq!(t.click((b.x + BTN / 2.0, M_TOP + PAD_Y / 2.0)), None);
        assert_eq!(t.click((1.0, 1.0)), None);
        assert_eq!(t.click((b.x + BTN / 2.0, top + BTN + 1.0)), None);
        assert_eq!(t.click((-5.0, -5.0)), None);
        let (w, h) = t.size();
        assert_eq!(t.click((w as f32 + 1.0, h as f32 / 2.0)), None);
    }

    #[test]
    fn buttons_are_inside_the_bar() {
        let t = Toolbar::new(PINNED, "coral", true, 1.0, Theme::FALLBACK);
        let first = t.buttons.first().unwrap().x;
        let last = t.buttons.last().unwrap().x + BTN;
        assert!(first - t.bar_x >= PAD_X - 0.01);
        assert!(t.bar_x + t.bar_w - last >= PAD_X - 0.01);
        assert!(t.bar_x >= M_SIDE && t.bar_x + t.bar_w <= t.w as f32 - M_SIDE);
    }

    #[test]
    fn unpinned_current_gets_a_button() {
        let mut t = Toolbar::new(PINNED, "coral", true, 1.0, Theme::FALLBACK);
        let w0 = t.size().0;
        t.set_current("mint");
        assert_eq!(t.buttons.len(), PINNED.len() + 3);
        assert_eq!(t.click(centre(&t, PINNED.len())), Some(Action::Hand("mint".into())));
        assert_eq!(t.click(centre(&t, PINNED.len() + 1)), Some(Action::ToggleSound));
        assert!(t.size().0 >= w0);
        t.set_current("robot");
        assert_eq!(t.buttons.len(), PINNED.len() + 2);
        assert_eq!(t.size().0, w0);
    }

    #[test]
    fn hover_reports_changes() {
        let mut t = Toolbar::new(PINNED, "coral", true, 1.0, Theme::FALLBACK);
        let p = centre(&t, 2);
        assert!(t.hover(Some(p)));
        assert!(!t.hover(Some((p.0 + 1.0, p.1))));
        assert!(t.hover(Some(centre(&t, 3))));
        assert!(t.hover(None));
        assert!(!t.hover(None));
        assert!(!t.hover(Some((0.0, 0.0))));
    }

    #[test]
    fn fast_bar_matches_reference() {
        for &s in &[1.0f32, 1.25, 1.5, 1.6, 2.0] {
            let t = Toolbar::new(PINNED, "coral", true, s, Theme::FALLBACK);
            let (bw, bh) = t.buffer_size();
            let (mut a, mut b) = (vec![0; bw * bh], vec![0; bw * bh]);
            t.draw_bar(&mut a, bw, bh, true);
            t.draw_bar(&mut b, bw, bh, false);
            for (p, q) in a.iter().zip(&b) {
                for sh in [0, 8, 16, 24] {
                    let d = ((p >> sh) & 0xFF) as i32 - ((q >> sh) & 0xFF) as i32;
                    assert!(d.abs() <= 1, "scale {s}: {p:08x} vs {q:08x}");
                }
            }
        }
    }

    #[test]
    fn render_fills_the_buffer() {
        // No hand images needed: missing ones fall back to a dot.
        let mut t = Toolbar::new(&["no-such-hand", "laser"], "laser", false, 1.25, Theme::FALLBACK);
        let (bw, bh) = t.buffer_size();
        let mut buf = vec![0xDEADBEEF; bw * bh];
        t.render(&mut buf);
        assert!(buf.iter().all(|&p| p != 0xDEADBEEF));
        assert_eq!(buf[0] >> 24, 0, "corner of the shadow margin is transparent");
        let (x, y, w, h) = t.body_rect();
        let mid = ((y + h / 2) as f32 * 1.25) as usize * bw + ((x + w / 2) as f32 * 1.25) as usize;
        assert!(buf[mid] >> 24 > 200);
        for p in &buf {
            let a = p >> 24;
            assert!((p >> 16 & 0xFF) <= a && (p >> 8 & 0xFF) <= a && (p & 0xFF) <= a, "premultiplied");
        }
    }
}
