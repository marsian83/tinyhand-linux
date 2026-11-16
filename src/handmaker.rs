//! Make your own hand (port of HandMaker.swift): the user asks an image tool for a hand
//! with `prompt()`, then `make()` checks the PNG it gives back, puts the hand on a stick
//! in the same layout as the built-in hands and picks an ink colour; `add()` saves it.
//!
//! Custom hands live in `$XDG_DATA_HOME/tinyhand/custom/` (usually
//! ~/.local/share/tinyhand/custom/): `<id>.png` per hand plus `hands.txt`, one
//! `id<TAB>#rrggbb<TAB>name` line per hand. Ids look like `custom-1a2b3c4d`.
//!
//! INTERFACE CONTRACT: the settings window codes against these signatures. Keep them.

use std::{
    fs::File,
    io::{BufReader, Read},
    path::{Path, PathBuf},
};

/// A user-made hand from `hands.txt`.
#[derive(Clone, Debug, PartialEq)]
pub struct CustomHand {
    /// `custom-xxxxxxxx`, usable anywhere a hand id is (config, `tinyhand hand ID`, toolbar).
    pub id: String,
    /// Display name, at most 16 characters.
    pub name: String,
    /// Ink colour 0xRRGGBB.
    pub ink: u32,
}

/// A hand that passed the checks and sits on its stick, ready to preview or save.
pub struct Made {
    /// Composed image, straight (not premultiplied) RGBA8, row-major.
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Ink colour picked from the hand, 0xRRGGBB.
    pub ink: u32,
}

const PROMPT: &str = "\
Make a pointer image for TinyHand, a desktop app that shows a toy hand on screen.

Draw one cartoon right hand in this style: [describe your hand, e.g. knitted wool in mustard yellow]

Rules:
- We see the BACK of the hand. The palm faces away from us. The knuckles and the smooth back of the hand face the viewer.
- The index finger points straight up. The other three fingers are folded down, so we only see their knuckles, never their fingertips or fingernails.
- The raised index finger sits at the far left of the knuckle row, right next to the thumb. Three folded knuckles sit to its right. It must never look like a middle finger.
- The thumb is tucked against the left side of the hand. Nothing crosses the front of the fist: no thumb, no fingertips, no fingernails, no palm.
- The fingertip is the highest point of the image. Nothing above or beside it.
- The hand ends at the bottom in a wrist cuff or a clean flat wrist, facing straight down, centred.
- Upright, not tilted. Whole hand visible, nothing cropped.
- Transparent background. No shadow, no floor, no outline glow, no text, no stick, no arm.
- Soft 3D toy look, bright and readable at small sizes.
- Portrait image, hand filling most of the height with a little empty space around it.";

const PLACEHOLDER: &str = "[describe your hand, e.g. knitted wool in mustard yellow]";

const UNREADABLE: &str = "That file isn't an image TinyHand can read.";
const NOT_TRANSPARENT: &str = "The image needs a transparent background. Save it as a PNG with transparency.";
const EMPTY: &str = "The image is empty.";

/// Longest name kept, in characters.
const NAME_MAX: usize = 16;

/// The prompt to paste into an image tool. `idea` fills in the style ("knitted wool in
/// mustard yellow"); empty uses the default style.
pub fn prompt(idea: &str) -> String {
    let idea = idea.trim();
    let style = if idea.is_empty() { "a soft vinyl toy in a bright colour" } else { idea };
    PROMPT.replace(PLACEHOLDER, style)
}

/// Check an image and put its hand on a stick. Err is a friendly sentence for the user
/// (e.g. "The image needs a transparent background. Save it as a PNG with transparency.").
pub fn make(path: &Path) -> Result<Made, String> {
    let p = read(path)?;
    make_from(&p)
}

fn make_from(p: &Pixels) -> Result<Made, String> {
    check(p)?;
    let rgba = compose(p).ok_or("TinyHand couldn't put that hand on a stick.")?;
    Ok(Made { width: CANVAS_W as u32, height: CANVAS_H as u32, rgba, ink: ink(p) })
}

/// Encode `made` as PNG bytes (for previews or saving elsewhere).
pub fn encode_png(made: &Made) -> Result<Vec<u8>, String> {
    let fail = |_| "TinyHand couldn't save the hand.".to_string();
    if made.rgba.len() != made.width as usize * made.height as usize * 4 {
        return Err("TinyHand couldn't save the hand.".into());
    }
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, made.width, made.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Fast);
    let mut w = enc.write_header().map_err(fail)?;
    w.write_image_data(&made.rgba).map_err(fail)?;
    w.finish().map_err(fail)?;
    Ok(out)
}

/// Save a made hand under `name` (trimmed, at most 16 chars; empty → "My hand N").
/// Returns the new hand.
pub fn add(made: &Made, name: &str) -> Result<CustomHand, String> {
    add_in(&dir(), made, name)
}

/// Delete a custom hand (its PNG and its line). Unknown ids are ignored.
pub fn remove(id: &str) -> Result<(), String> {
    remove_in(&dir(), id)
}

/// Rename a custom hand.
pub fn rename(id: &str, name: &str) -> Result<(), String> {
    rename_in(&dir(), id, name)
}

/// All custom hands, in the order they were added.
pub fn list() -> Vec<CustomHand> {
    list_in(&dir())
}

/// The custom hand with this id, if any.
pub fn get(id: &str) -> Option<CustomHand> {
    if !valid_id(id) {
        return None;
    }
    list().into_iter().find(|h| h.id == id)
}

/// Where custom hands are stored (`$TINYHAND_CUSTOM_DIR` overrides, for tests).
pub fn dir() -> PathBuf {
    if let Some(d) = std::env::var_os("TINYHAND_CUSTOM_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    let data = std::env::var_os("XDG_DATA_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    data.join("tinyhand/custom")
}

/// A name suggested from the image's file name ("knit_wool.png" → "Knit wool"), or
/// "My hand N" for names like "ChatGPT Image …".
pub fn suggested_name(path: &Path) -> String {
    suggested_name_for(path, list().len())
}

// ---------------------------------------------------------------------------------------
// Reading and checking

/// Straight (not premultiplied) RGBA8.
struct Pixels {
    w: usize,
    h: usize,
    rgba: Vec<u8>,
}

impl Pixels {
    #[inline]
    fn alpha(&self, x: usize, y: usize) -> u8 {
        self.rgba[(y * self.w + x) * 4 + 3]
    }
}

#[derive(Clone, Copy, Debug)]
struct Bounds {
    min_x: usize,
    min_y: usize,
    max_x: usize,
    max_y: usize,
}

fn read(path: &Path) -> Result<Pixels, String> {
    let file = File::open(path).map_err(|_| UNREADABLE.to_string())?;
    let mut dec = png::Decoder::new(BufReader::new(file));
    // 8-bit output; palettes and tRNS chunks become real alpha channels.
    dec.set_transformations(png::Transformations::normalize_to_color8());
    let too_big = |e: png::DecodingError| match e {
        png::DecodingError::LimitsExceeded => {
            "The image is too big. Use one under 4000 pixels on each side.".to_string()
        }
        _ => UNREADABLE.to_string(),
    };
    let mut reader = dec.read_info().map_err(too_big)?;
    let (color, _) = reader.output_color_type();
    if !matches!(color, png::ColorType::Rgba | png::ColorType::GrayscaleAlpha) {
        return Err(NOT_TRANSPARENT.into());
    }
    let (w, h) = (reader.info().width as usize, reader.info().height as usize);
    if w < 64 || h < 64 {
        return Err(format!("The image is too small ({w}×{h}). Use one at least 512 pixels tall and 64 wide."));
    }
    let mut buf = vec![0; reader.output_buffer_size().ok_or_else(|| too_big(png::DecodingError::LimitsExceeded))?];
    let info = reader.next_frame(&mut buf).map_err(too_big)?;
    let rgba = match info.color_type {
        png::ColorType::Rgba => {
            buf.truncate(w * h * 4);
            buf
        }
        png::ColorType::GrayscaleAlpha => {
            buf[..w * h * 2].as_chunks::<2>().0.iter().flat_map(|p| [p[0], p[0], p[0], p[1]]).collect()
        }
        _ => return Err(NOT_TRANSPARENT.into()),
    };
    if rgba.len() != w * h * 4 {
        return Err(UNREADABLE.into());
    }
    Ok(Pixels { w, h, rgba })
}

/// Same checks, thresholds and messages as `HandMaker.check` in the macOS app.
fn check(p: &Pixels) -> Result<(), String> {
    let (w, h) = (p.w, p.h);
    let solid = |x, y| (p.alpha(x, y) > 32) as usize;
    let mut edge = 0;
    let mut edge_solid = 0;
    for x in 0..w {
        edge += 2;
        edge_solid += solid(x, 0) + solid(x, h - 1);
    }
    for y in 0..h {
        edge += 2;
        edge_solid += solid(0, y) + solid(w - 1, y);
    }
    if edge_solid as f64 / edge as f64 > 0.02 {
        return Err("The background isn't transparent, or the hand touches the edge of the image.".into());
    }

    // Connected pieces on a grid of at most ~256 cells along the short side.
    let step = (w.min(h) / 256).max(1);
    let (gw, gh) = (w / step, h / step);
    let grid: Vec<bool> = (0..gw * gh).map(|i| p.alpha((i % gw) * step, (i / gw) * step) > 128).collect();
    let mut seen = vec![false; gw * gh];
    let (mut biggest, mut total) = (0usize, 0usize);
    let mut stack = Vec::new();
    for start in 0..grid.len() {
        if !grid[start] || seen[start] {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        let mut n = 0;
        while let Some(i) = stack.pop() {
            n += 1;
            let (x, y) = (i % gw, i / gw);
            let mut visit = |j: usize| {
                if grid[j] && !seen[j] {
                    seen[j] = true;
                    stack.push(j);
                }
            };
            if x + 1 < gw {
                visit(i + 1);
            }
            if x > 0 {
                visit(i - 1);
            }
            if y + 1 < gh {
                visit(i + gw);
            }
            if y > 0 {
                visit(i - gw);
            }
        }
        biggest = biggest.max(n);
        total += n;
    }
    if total == 0 {
        return Err(EMPTY.into());
    }
    if (biggest as f64) / (total as f64) < 0.97 {
        return Err("The image has more than one thing in it. Use a single hand.".into());
    }

    let b = bounds(p).ok_or(EMPTY)?;
    let bw = b.max_x - b.min_x + 1;
    let bh = b.max_y - b.min_y + 1;
    let aspect = bh as f64 / bw as f64;
    if !(1.05..=2.8).contains(&aspect) {
        return Err("The hand should stand upright, taller than it is wide.".into());
    }
    if (bh as f64) / (h as f64) < 0.5 {
        return Err("The hand is too small in the image. Crop it closer.".into());
    }

    let top_band = (bh * 8 / 100).max(2);
    let (mut finger_min, mut finger_max) = (w as isize, 0isize);
    for y in b.min_y..(b.min_y + top_band).min(h) {
        for x in b.min_x..=b.max_x {
            if p.alpha(x, y) > 128 {
                finger_min = finger_min.min(x as isize);
                finger_max = finger_max.max(x as isize);
            }
        }
    }
    if (finger_max - finger_min + 1) as f64 / bw as f64 > 0.4 {
        return Err("The top of the image should be a single finger pointing up.".into());
    }

    let (base_w, _) = bottom_span(p, b);
    if (base_w as f64) / (bw as f64) < 0.2 {
        return Err("The hand should end in a flat wrist or cuff at the bottom.".into());
    }

    let (mut soft, mut any) = (0usize, 0usize);
    for y in (0..h).step_by(2) {
        for x in (0..w).step_by(2) {
            let a = p.alpha(x, y);
            if a > 20 {
                any += 1;
                if a < 200 {
                    soft += 1;
                }
            }
        }
    }
    if soft as f64 / any.max(1) as f64 > 0.08 {
        return Err("The hand has a soft shadow or glow around it. Ask for no shadow.".into());
    }
    Ok(())
}

/// Bounding box of the pixels more than half opaque.
fn bounds(p: &Pixels) -> Option<Bounds> {
    let mut b = Bounds { min_x: p.w, min_y: p.h, max_x: 0, max_y: 0 };
    let mut any = false;
    for y in 0..p.h {
        let row = &p.rgba[y * p.w * 4..(y + 1) * p.w * 4];
        let Some(lo) = row.as_chunks::<4>().0.iter().position(|px| px[3] > 128) else { continue };
        let hi = row.as_chunks::<4>().0.iter().rposition(|px| px[3] > 128).unwrap_or(lo);
        any = true;
        b.min_x = b.min_x.min(lo);
        b.max_x = b.max_x.max(hi);
        b.min_y = b.min_y.min(y);
        b.max_y = y;
    }
    any.then_some(b)
}

/// Width and centre x of the hand's bottom 4% (the wrist or cuff).
fn bottom_span(p: &Pixels, b: Bounds) -> (usize, f64) {
    let bh = b.max_y - b.min_y + 1;
    let (mut lo, mut hi) = (p.w, None);
    let from = b.min_y.max(b.max_y.saturating_sub((bh * 4 / 100).max(2)));
    for y in from..=b.max_y {
        for x in b.min_x..=b.max_x {
            if p.alpha(x, y) > 128 {
                lo = lo.min(x);
                hi = Some(hi.map_or(x, |h: usize| h.max(x)));
            }
        }
    }
    match hi {
        Some(hi) => (hi - lo + 1, (lo + hi) as f64 / 2.0),
        None => (0, (b.min_x + b.max_x) as f64 / 2.0),
    }
}

// ---------------------------------------------------------------------------------------
// Composing: the same canvas and layout as the built-in hands (1024×1536, fingertip at
// y = 150, hand 560 px tall, wrist centred, a dark rounded rod down to y = 1462).

const CANVAS_W: usize = 1024;
const CANVAS_H: usize = 1536;
const HAND_TOP: f64 = 150.0;
const HAND_HEIGHT: f64 = 560.0;
const HAND_MAX_W: f64 = 700.0;
const ROD_BOTTOM: f64 = 1462.0;

/// Returns the composed canvas as straight RGBA8.
fn compose(p: &Pixels) -> Option<Vec<u8>> {
    let b = bounds(p)?;
    let bw = (b.max_x - b.min_x + 1) as f64;
    let bh = (b.max_y - b.min_y + 1) as f64;
    let scale = (HAND_HEIGHT / bh).min(HAND_MAX_W / bw);
    let wrist_x = (bottom_span(p, b).1 + 0.5) * scale;
    let origin_x = CANVAS_W as f64 / 2.0 - wrist_x;
    let origin_top = HAND_TOP - b.min_y as f64 * scale;
    let hand_bottom = HAND_TOP + bh * scale;

    // Premultiplied RGBA in 0..255.
    let mut canvas = vec![[0f32; 4]; CANVAS_W * CANVAS_H];

    // The rod: a pill with a dark horizontal gradient, under the hand.
    let rod_w = (bw * scale * 0.18).clamp(54.0, 76.0);
    let rod_top = hand_bottom - 30.0;
    let rod_x0 = CANVAS_W as f64 / 2.0 - rod_w / 2.0;
    let r = rod_w / 2.0;
    let (cx, cy) = (CANVAS_W as f64 / 2.0, (rod_top + ROD_BOTTOM) / 2.0);
    let (hw, hh) = (rod_w / 2.0, (ROD_BOTTOM - rod_top) / 2.0);
    let ys = (rod_top.floor().max(0.0) as usize)..(ROD_BOTTOM.ceil() as usize).min(CANVAS_H);
    let xs = (rod_x0.floor().max(0.0) as usize)..((rod_x0 + rod_w).ceil() as usize).min(CANVAS_W);
    for y in ys {
        for x in xs.clone() {
            let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
            // Signed distance to the rounded rectangle.
            let qx = (px - cx).abs() - (hw - r);
            let qy = (py - cy).abs() - (hh - r);
            let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
            let d = outside + qx.max(qy).min(0.0) - r;
            let cov = (0.5 - d).clamp(0.0, 1.0) as f32;
            if cov <= 0.0 {
                continue;
            }
            let c = rod_colour(((px - rod_x0) / rod_w) as f32);
            canvas[y * CANVAS_W + x] = [c[0] * cov, c[1] * cov, c[2] * cov, 255.0 * cov];
        }
    }

    // The hand, resampled into place and drawn over the rod.
    let draw_w = p.w as f64 * scale;
    let draw_h = p.h as f64 * scale;
    let x_out = (origin_x.floor().max(0.0) as usize)..((origin_x + draw_w).ceil().max(0.0) as usize).min(CANVAS_W);
    let y_out = (origin_top.floor().max(0.0) as usize)..((origin_top + draw_h).ceil().max(0.0) as usize).min(CANVAS_H);
    if x_out.is_empty() || y_out.is_empty() {
        return None;
    }
    let xt = taps(p.w, scale, origin_x, x_out.clone());
    let yt = taps(p.h, scale, origin_top, y_out.clone());
    let ow = x_out.len();

    // Horizontal pass, only for the source rows the vertical pass reads.
    let src_rows = yt.iter().map(|t| t.0).min().unwrap_or(0)..yt.iter().map(|t| t.0 + t.1.len()).max().unwrap_or(0);
    let mut horiz = vec![[0f32; 4]; ow * src_rows.len()];
    for (ry, y) in src_rows.clone().enumerate() {
        let row = &p.rgba[y * p.w * 4..(y + 1) * p.w * 4];
        if row.as_chunks::<4>().0.iter().all(|px| px[3] == 0) {
            continue;
        }
        let out = &mut horiz[ry * ow..(ry + 1) * ow];
        for (o, (lo, ws)) in xt.iter().enumerate() {
            let mut acc = [0f32; 4];
            for (j, &wt) in ws.iter().enumerate() {
                let s = &row[(lo + j) * 4..][..4];
                let a = s[3] as f32 * wt;
                if a == 0.0 {
                    continue;
                }
                let k = a / 255.0;
                acc[0] += s[0] as f32 * k;
                acc[1] += s[1] as f32 * k;
                acc[2] += s[2] as f32 * k;
                acc[3] += a;
            }
            out[o] = acc;
        }
    }
    for (oy, (lo, ws)) in yt.iter().enumerate() {
        let y = y_out.start + oy;
        for ox in 0..ow {
            let mut acc = [0f32; 4];
            for (j, &wt) in ws.iter().enumerate() {
                let s = horiz[(lo + j - src_rows.start) * ow + ox];
                for c in 0..4 {
                    acc[c] += s[c] * wt;
                }
            }
            if acc[3] <= 0.0 {
                continue;
            }
            let d = &mut canvas[y * CANVAS_W + x_out.start + ox];
            let ia = 1.0 - (acc[3] / 255.0).min(1.0);
            for c in 0..4 {
                d[c] = acc[c] + d[c] * ia;
            }
        }
    }

    // Back to straight RGBA8; fully transparent pixels stay all zero.
    let mut out = vec![0u8; CANVAS_W * CANVAS_H * 4];
    for (o, c) in out.as_chunks_mut::<4>().0.iter_mut().zip(&canvas) {
        let a = c[3].round().clamp(0.0, 255.0);
        if a <= 0.0 {
            continue;
        }
        let k = 255.0 / c[3];
        o[0] = (c[0] * k).round().clamp(0.0, 255.0) as u8;
        o[1] = (c[1] * k).round().clamp(0.0, 255.0) as u8;
        o[2] = (c[2] * k).round().clamp(0.0, 255.0) as u8;
        o[3] = a as u8;
    }
    Some(out)
}

/// The rod's gradient across its width, `t` in 0..1 (black, dark grey, a soft highlight
/// left of centre, dark grey, black).
fn rod_colour(t: f32) -> [f32; 3] {
    const STOPS: [(f32, [f32; 3]); 5] = [
        (0.0, [0.0, 0.0, 0.0]),
        (0.22, [18.0, 18.0, 20.0]),
        (0.45, [58.0, 58.0, 62.0]),
        (0.75, [18.0, 18.0, 20.0]),
        (1.0, [0.0, 0.0, 0.0]),
    ];
    let t = t.clamp(0.0, 1.0);
    let i = STOPS.iter().rposition(|s| s.0 <= t).unwrap_or(0).min(STOPS.len() - 2);
    let ((t0, a), (t1, b)) = (STOPS[i], STOPS[i + 1]);
    let f = ((t - t0) / (t1 - t0)).clamp(0.0, 1.0);
    [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f, a[2] + (b[2] - a[2]) * f]
}

/// Resampling taps for drawing `len` source pixels scaled by `scale` starting at `origin`
/// (in output pixels): for each output pixel in `out`, the first source index and the
/// weights. A triangle filter (bilinear when enlarging, area-like when shrinking);
/// samples outside the image count as transparent.
fn taps(len: usize, scale: f64, origin: f64, out: std::ops::Range<usize>) -> Vec<(usize, Vec<f32>)> {
    let support = if scale < 1.0 { 1.0 / scale } else { 1.0 };
    out.map(|o| {
        let c = (o as f64 + 0.5 - origin) / scale;
        let lo = (c - support).floor() as isize;
        let hi = (c + support).ceil() as isize;
        let tri = |i: isize| (1.0 - ((i as f64 + 0.5 - c) / support).abs()).max(0.0);
        let sum: f64 = (lo..hi).map(tri).sum();
        let first = lo.max(0) as usize;
        let last = (hi.max(0) as usize).min(len);
        let ws = if sum > 0.0 && first < last {
            (first..last).map(|i| (tri(i as isize) / sum) as f32).collect()
        } else {
            Vec::new()
        };
        (first.min(len), ws)
    })
    .collect()
}

/// Ink colour: the most common saturated, mid-bright colour of the hand (same buckets
/// and scoring as `HandMaker.ink`).
fn ink(p: &Pixels) -> u32 {
    #[derive(Clone, Copy, Default)]
    struct Bucket {
        n: u32,
        r: f64,
        g: f64,
        b: f64,
    }
    let mut buckets = [Bucket::default(); 216];
    for y in (0..p.h).step_by(3) {
        for x in (0..p.w).step_by(3) {
            let px = &p.rgba[(y * p.w + x) * 4..][..4];
            if px[3] <= 200 {
                continue;
            }
            let (r, g, b) = (px[0] as f64 / 255.0, px[1] as f64 / 255.0, px[2] as f64 / 255.0);
            let q = |v: f64| ((v * 5.0) as usize).min(5);
            let e = &mut buckets[q(r) * 36 + q(g) * 6 + q(b)];
            e.n += 1;
            e.r += r;
            e.g += g;
            e.b += b;
        }
    }
    let (mut best_score, mut best) = (-1.0, (0.53, 0.33, 0.95));
    for e in buckets.iter().filter(|e| e.n > 0) {
        let n = e.n as f64;
        let (r, g, b) = (e.r / n, e.g / n, e.b / n);
        let hi = r.max(g).max(b);
        let lo = r.min(g).min(b);
        let sat = if hi == 0.0 { 0.0 } else { (hi - lo) / hi };
        let lum = 0.3 * r + 0.59 * g + 0.11 * b;
        let score = n * (0.25 + sat) * if lum > 0.12 && lum < 0.85 { 1.0 } else { 0.15 };
        if score > best_score {
            best_score = score;
            best = (r, g, b);
        }
    }
    let (mut r, mut g, mut b) = best;
    let hi = r.max(g).max(b);
    let lo = r.min(g).min(b);
    if hi - lo < 0.08 {
        (r, g, b) = (0.42, 0.45, 0.5);
    } else if hi > 0.82 {
        let k = 0.82 / hi;
        (r, g, b) = (r * k, g * k, b * k);
    }
    let c = |v: f64| (v * 255.0).clamp(0.0, 255.0) as u32;
    c(r) << 16 | c(g) << 8 | c(b)
}

// ---------------------------------------------------------------------------------------
// Storage

const LIST: &str = "hands.txt";

/// `custom-` followed by letters, digits, `-` or `_` (so it's safe as a file name).
fn valid_id(id: &str) -> bool {
    id.strip_prefix("custom-").is_some_and(|rest| {
        !rest.is_empty() && rest.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    })
}

/// Trimmed, single-line, at most `NAME_MAX` characters.
fn clean_name(name: &str) -> String {
    let flat: String = name.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    flat.trim().chars().take(NAME_MAX).collect::<String>().trim_end().to_string()
}

fn list_in(dir: &Path) -> Vec<CustomHand> {
    let Ok(text) = std::fs::read_to_string(dir.join(LIST)) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| {
            let mut f = line.splitn(3, '\t');
            let id = f.next()?.trim();
            let ink = u32::from_str_radix(f.next()?.trim().strip_prefix('#')?, 16).ok()?;
            let name = f.next().unwrap_or("").trim();
            valid_id(id).then(|| CustomHand { id: id.into(), name: name.into(), ink: ink & 0xFFFFFF })
        })
        .collect()
}

fn save_list(dir: &Path, list: &[CustomHand]) -> Result<(), String> {
    let text: String = list.iter().map(|h| format!("{}\t#{:06x}\t{}\n", h.id, h.ink, clean_name(&h.name))).collect();
    write_atomic(&dir.join(LIST), text.as_bytes())
}

/// Write to a temporary file next to `path`, then rename it into place.
fn write_atomic(path: &Path, data: &[u8]) -> Result<(), String> {
    crate::config::write_atomic(path, data).map_err(|e| format!("TinyHand couldn't save the hand ({e})."))
}

/// 8 random hex digits from /dev/urandom, or from the clock and pid without it.
fn random_hex() -> String {
    let mut b = [0u8; 4];
    if File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).is_ok() {
        return format!("{:08x}", u32::from_le_bytes(b));
    }
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    // splitmix64 of the clock and pid.
    let mut x = (t.as_nanos() as u64) ^ (std::process::id() as u64) << 40;
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
    format!("{:08x}", (x ^ (x >> 31)) as u32)
}

fn add_in(dir: &Path, made: &Made, name: &str) -> Result<CustomHand, String> {
    let png = encode_png(made)?;
    let mut list = list_in(dir);
    let id = loop {
        let id = format!("custom-{}", random_hex());
        if !list.iter().any(|h| h.id == id) && !dir.join(format!("{id}.png")).exists() {
            break id;
        }
    };
    write_atomic(&dir.join(format!("{id}.png")), &png)?;
    let name = clean_name(name);
    let name = if name.is_empty() { format!("My hand {}", list.len() + 1) } else { name };
    let hand = CustomHand { id, name, ink: made.ink & 0xFFFFFF };
    list.push(hand.clone());
    if let Err(e) = save_list(dir, &list) {
        let _ = std::fs::remove_file(dir.join(format!("{}.png", hand.id)));
        return Err(e);
    }
    Ok(hand)
}

fn remove_in(dir: &Path, id: &str) -> Result<(), String> {
    if !valid_id(id) {
        return Ok(());
    }
    let list = list_in(dir);
    if list.iter().any(|h| h.id == id) {
        let rest: Vec<CustomHand> = list.into_iter().filter(|h| h.id != id).collect();
        save_list(dir, &rest).map_err(|_| "TinyHand couldn't remove the hand.".to_string())?;
    }
    match std::fs::remove_file(dir.join(format!("{id}.png"))) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("TinyHand couldn't remove the hand ({e}).")),
        _ => Ok(()),
    }
}

fn rename_in(dir: &Path, id: &str, name: &str) -> Result<(), String> {
    let name = clean_name(name);
    if name.is_empty() {
        return Err("Give the hand a name.".into());
    }
    let mut list = list_in(dir);
    let hand = list.iter_mut().find(|h| h.id == id).ok_or("That hand isn't there any more.")?;
    if hand.name == name {
        return Ok(());
    }
    hand.name = name;
    save_list(dir, &list)
}

fn suggested_name_for(path: &Path, count: usize) -> String {
    let base = path.file_stem().map(|s| s.to_string_lossy().replace(['_', '-'], " ")).unwrap_or_default();
    let words = base.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
    if words.is_empty() || words.chars().count() > NAME_MAX || words.to_ascii_lowercase().starts_with("chatgpt image") {
        return format!("My hand {}", count + 1);
    }
    let mut c = words.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or(words)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serialises tests that set `TINYHAND_CUSTOM_DIR` (tests run in parallel).
    pub(crate) static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// A fresh empty folder for one test.
    pub(crate) fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("tinyhand-test-{tag}-{}-{}", std::process::id(), random_hex()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn blank(w: usize, h: usize) -> Pixels {
        Pixels { w, h, rgba: vec![0; w * h * 4] }
    }

    fn fill(p: &mut Pixels, x0: usize, y0: usize, x1: usize, y1: usize, rgb: [u8; 3]) {
        for y in y0..y1 {
            for x in x0..x1 {
                p.rgba[(y * p.w + x) * 4..][..4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
            }
        }
    }

    /// A blocky orange hand: a finger up the left, a fist, and a cuff at the bottom.
    pub(crate) fn synthetic_hand() -> (usize, usize, Vec<u8>) {
        let mut p = blank(512, 768);
        fill(&mut p, 150, 60, 230, 330, [240, 120, 40]); // finger
        fill(&mut p, 140, 300, 400, 600, [230, 110, 30]); // fist
        fill(&mut p, 300, 320, 390, 400, [200, 90, 20]); // a knuckle shade
        fill(&mut p, 180, 600, 360, 700, [90, 60, 200]); // cuff
        (p.w, p.h, p.rgba)
    }

    pub(crate) fn write_png(path: &Path, w: usize, h: usize, color: png::ColorType, data: &[u8]) {
        let f = File::create(path).unwrap();
        let mut enc = png::Encoder::new(std::io::BufWriter::new(f), w as u32, h as u32);
        enc.set_color(color);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(data).unwrap();
    }

    fn to_image(m: &Made) -> crate::hand::Image {
        crate::hand::Image { w: m.width as usize, h: m.height as usize, rgba: m.rgba.clone() }
    }

    #[test]
    fn prompt_fills_in_the_style() {
        let p = prompt("  knitted wool in mustard yellow \n");
        assert!(p.contains("this style: knitted wool in mustard yellow\n"));
        assert!(p.contains("a desktop app"));
        assert!(!p.contains("Mac") && !p.contains('['));
        assert!(prompt("").contains("this style: a soft vinyl toy in a bright colour\n"));
    }

    #[test]
    fn rejects_opaque_images() {
        let d = temp_dir("opaque");
        let rgb = vec![200u8; 300 * 400 * 3];
        write_png(&d.join("a.png"), 300, 400, png::ColorType::Rgb, &rgb);
        assert_eq!(make(&d.join("a.png")).err().unwrap(), NOT_TRANSPARENT);
        let gray = vec![200u8; 300 * 400];
        write_png(&d.join("g.png"), 300, 400, png::ColorType::Grayscale, &gray);
        assert_eq!(make(&d.join("g.png")).err().unwrap(), NOT_TRANSPARENT);
        // RGBA but solid everywhere: has alpha, fails on the edges instead.
        let solid = vec![255u8; 300 * 400 * 4];
        write_png(&d.join("s.png"), 300, 400, png::ColorType::Rgba, &solid);
        assert!(make(&d.join("s.png")).err().unwrap().starts_with("The background isn't transparent"));
        std::fs::write(d.join("x.png"), b"not a png").unwrap();
        assert_eq!(make(&d.join("x.png")).err().unwrap(), UNREADABLE);
        assert_eq!(make(&d.join("missing.png")).err().unwrap(), UNREADABLE);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn rejects_tiny_images() {
        let d = temp_dir("tiny");
        write_png(&d.join("t.png"), 40, 60, png::ColorType::Rgba, &vec![0u8; 40 * 60 * 4]);
        assert!(make(&d.join("t.png")).err().unwrap().starts_with("The image is too small"));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn rejects_shapes_that_arent_a_hand() {
        let err = |p: &Pixels| check(p).err().unwrap();
        assert_eq!(err(&blank(200, 300)), EMPTY);
        let mut two = blank(400, 600);
        fill(&mut two, 50, 50, 150, 550, [255, 0, 0]);
        fill(&mut two, 250, 50, 350, 550, [255, 0, 0]);
        assert!(err(&two).contains("more than one thing"));
        let mut wide = blank(600, 400);
        fill(&mut wide, 50, 50, 550, 350, [255, 0, 0]);
        assert!(err(&wide).contains("upright"));
        let mut small = blank(400, 800);
        fill(&mut small, 100, 100, 250, 300, [255, 0, 0]);
        assert!(err(&small).contains("too small in the image"));
        let mut block = blank(400, 600);
        fill(&mut block, 100, 50, 300, 550, [255, 0, 0]);
        assert!(err(&block).contains("single finger"));
        // A finger on a fist that tapers to a point (no wrist).
        let mut point = blank(400, 600);
        fill(&mut point, 120, 40, 170, 200, [255, 0, 0]);
        fill(&mut point, 100, 200, 300, 500, [255, 0, 0]);
        fill(&mut point, 190, 500, 210, 560, [255, 0, 0]);
        assert!(err(&point).contains("flat wrist"));
        // A good shape with a soft glow around it.
        let (w, h, rgba) = synthetic_hand();
        let mut glow = Pixels { w, h, rgba };
        for y in 20..h - 20 {
            for x in 20..w - 20 {
                let a = &mut glow.rgba[(y * w + x) * 4 + 3];
                if *a == 0 {
                    *a = 60;
                }
            }
        }
        assert!(err(&glow).contains("soft shadow"));
    }

    #[test]
    fn makes_a_hand_in_the_built_in_layout() {
        let d = temp_dir("make");
        let (w, h, rgba) = synthetic_hand();
        write_png(&d.join("hand.png"), w, h, png::ColorType::Rgba, &rgba);
        let made = make(&d.join("hand.png")).unwrap();
        assert_eq!((made.width, made.height), (1024, 1536));
        assert_eq!(made.rgba.len(), 1024 * 1536 * 4);
        // The orange body wins over the small purple cuff.
        let (r, g, b) = (made.ink >> 16, made.ink >> 8 & 255, made.ink & 255);
        assert!(r > g && g > b && r > 150, "ink {:06x}", made.ink);

        let m = crate::hand::measure(&to_image(&made)).expect("composed hand measures");
        assert!((m.tip - 150.0 / 1536.0).abs() < 0.005, "tip {}", m.tip);
        assert!((m.wrist - 710.0 / 1536.0).abs() < 0.01, "wrist {}", m.wrist);
        assert!(m.rod_end > 0.94 && m.rod_end < 0.955, "rod end {}", m.rod_end);
        assert!((m.rod_x - 0.5).abs() < 0.005, "rod x {}", m.rod_x);
        assert!(m.tip_x < 0.5 && m.tip_x > 0.3, "tip x {}", m.tip_x);
        // The same layout as a built-in hand.
        let built_in =
            crate::hand::load_png(&Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/hands/hand_lavender.png"));
        if let Some(b) = built_in.ok().and_then(|i| crate::hand::measure(&i)) {
            // The built-ins were drawn by hand, so they're close but not exact.
            assert!((b.tip - m.tip).abs() < 0.04, "built-in tip {} vs {}", b.tip, m.tip);
            assert!((b.wrist - m.wrist).abs() < 0.06, "built-in wrist {} vs {}", b.wrist, m.wrist);
            assert!((b.rod_x - m.rod_x).abs() < 0.02, "built-in rod x {} vs {}", b.rod_x, m.rod_x);
        }
        // A sprite builds from it like from any hand.
        let sprite = crate::hand::Sprite::build(&to_image(&made), m, 140);
        assert!(sprite.w > 0 && sprite.h > 140);

        // Gray-alpha input works too.
        let ga: Vec<u8> = rgba.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[3]]).collect();
        write_png(&d.join("ga.png"), w, h, png::ColorType::GrayscaleAlpha, &ga);
        let gray = make(&d.join("ga.png")).unwrap();
        assert_eq!(gray.ink, 0x6b727f, "a colourless hand gets slate ink");

        // Encoded output decodes and measures the same.
        let png_bytes = encode_png(&made).unwrap();
        std::fs::write(d.join("out.png"), &png_bytes).unwrap();
        let back = crate::hand::load_png(&d.join("out.png")).unwrap();
        assert_eq!(back.rgba, made.rgba);
        let _ = std::fs::remove_dir_all(d);
    }

    /// End to end on a real hand: a built-in hand with its stick cut off goes back on a stick.
    #[test]
    fn remakes_a_built_in_hand() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/hands/hand_coral.png");
        let Ok(img) = crate::hand::load_png(&path) else { return };
        let orig = crate::hand::measure(&img).unwrap();
        // Rows from just above the fingertip to just below the wrist: the hand alone, with
        // a little empty space around it, as an image tool would give it back.
        let y0 = (orig.tip * img.h as f32) as usize - 40;
        let cut = (orig.wrist * img.h as f32) as usize;
        let h = cut - y0 + 40;
        let mut rgba = vec![0u8; img.w * h * 4];
        rgba[..(cut - y0) * img.w * 4].copy_from_slice(&img.rgba[y0 * img.w * 4..cut * img.w * 4]);
        let p = Pixels { w: img.w, h, rgba };
        let t = std::time::Instant::now();
        let made = make_from(&p).unwrap();
        eprintln!("make() on {}x{} without decoding: {:?}", img.w, img.h, t.elapsed());
        let m = crate::hand::measure(&to_image(&made)).unwrap();
        eprintln!("built-in {orig:?}\nremade {m:?}");
        assert!((m.tip - 150.0 / 1536.0).abs() < 0.005, "tip {}", m.tip);
        assert!((m.tip - orig.tip).abs() < 0.04, "tip {} vs {}", m.tip, orig.tip);
        assert!((m.wrist - 710.0 / 1536.0).abs() < 0.01, "wrist {}", m.wrist);
        assert!((m.wrist - orig.wrist).abs() < 0.05, "wrist {} vs {}", m.wrist, orig.wrist);
        assert!((m.rod_x - orig.rod_x).abs() < 0.02, "rod x {} vs {}", m.rod_x, orig.rod_x);
        assert!(m.rod_end > 0.9);
        let (r, g, b) = (made.ink >> 16, made.ink >> 8 & 255, made.ink & 255);
        assert!(r > 150 && r > g + 60 && r > b + 60, "coral ink {:06x}", made.ink);

        let d = temp_dir("timing");
        write_png(&d.join("coral.png"), p.w, p.h, png::ColorType::Rgba, &p.rgba);
        let t = std::time::Instant::now();
        make(&d.join("coral.png")).unwrap();
        eprintln!("make() on {}x{} with decoding: {:?}", p.w, p.h, t.elapsed());
        let t = std::time::Instant::now();
        let bytes = encode_png(&made).unwrap();
        eprintln!("encode_png: {:?}, {} KB", t.elapsed(), bytes.len() / 1024);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn add_list_rename_remove() {
        let d = temp_dir("store");
        let made = Made { width: 2, height: 2, rgba: vec![0; 16], ink: 0x123456 };
        assert!(list_in(&d).is_empty());
        let a = add_in(&d, &made, "  Woolly\tthing with a long name ").unwrap();
        assert!(valid_id(&a.id) && a.id.len() == "custom-".len() + 8, "{}", a.id);
        assert_eq!(a.name, "Woolly thing wit");
        assert_eq!(a.ink, 0x123456);
        assert!(d.join(format!("{}.png", a.id)).exists());
        let b = add_in(&d, &made, "   ").unwrap();
        assert_eq!(b.name, "My hand 2");
        assert_ne!(a.id, b.id);
        assert_eq!(list_in(&d), vec![a.clone(), b.clone()]);
        assert_eq!(suggested_name_for(Path::new("ChatGPT Image Apr 3.png"), list_in(&d).len()), "My hand 3");

        rename_in(&d, &b.id, "Second\n").unwrap();
        assert_eq!(list_in(&d)[1].name, "Second");
        assert!(rename_in(&d, &b.id, "  ").is_err());
        assert!(rename_in(&d, "custom-nope", "x").is_err());

        remove_in(&d, &a.id).unwrap();
        assert!(!d.join(format!("{}.png", a.id)).exists());
        assert_eq!(list_in(&d).iter().map(|h| h.id.clone()).collect::<Vec<_>>(), vec![b.id.clone()]);
        remove_in(&d, "custom-unknown").unwrap();
        remove_in(&d, "../../etc/passwd").unwrap();
        assert_eq!(list_in(&d).len(), 1);
        // No temporary files left behind.
        let names: Vec<String> =
            std::fs::read_dir(&d).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into()).collect();
        assert!(names.iter().all(|n| !n.ends_with(".tmp")), "{names:?}");

        // Hand-edited or damaged lines are skipped.
        std::fs::write(
            d.join(LIST),
            "custom-ok\t#ff0000\tRed\nbad line\ncustom-../x\t#00ff00\tX\ncustom-noname\t#00ff00\n",
        )
        .unwrap();
        let l = list_in(&d);
        assert_eq!(l.len(), 2);
        assert_eq!((l[0].name.as_str(), l[0].ink), ("Red", 0xff0000));
        assert_eq!(l[1].name, "");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn public_api_uses_the_custom_dir() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let d = temp_dir("env");
        std::env::set_var("TINYHAND_CUSTOM_DIR", &d);
        assert_eq!(dir(), d);
        let made = Made { width: 2, height: 2, rgba: vec![0; 16], ink: 0xabcdef };
        let h = add(&made, "Mine").unwrap();
        assert_eq!(get(&h.id), Some(h.clone()));
        assert_eq!(list(), vec![h.clone()]);
        assert_eq!(crate::hand::find(&h.id), Some(d.join(format!("{}.png", h.id))));
        assert_eq!(crate::config::hand_title(&h.id).as_deref(), Some("Mine"));
        let c = crate::config::Config { hand: h.id.clone(), ..Default::default() };
        assert_eq!(c.ink_colour(), 0xabcdef);
        let all = crate::config::all_hands();
        assert_eq!(all.last().map(|h| h.0.as_str()), Some("laser"));
        assert_eq!(all[all.len() - 2], (h.id.clone(), "Mine".to_string()));
        rename(&h.id, "Renamed").unwrap();
        assert_eq!(get(&h.id).unwrap().name, "Renamed");
        remove(&h.id).unwrap();
        assert_eq!(get(&h.id), None);
        assert_eq!(crate::hand::find(&h.id), None);
        assert_eq!(suggested_name(Path::new("ChatGPT Image.png")), "My hand 1");
        std::env::remove_var("TINYHAND_CUSTOM_DIR");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn suggests_names() {
        let s = |p: &str| suggested_name_for(Path::new(p), 2);
        assert_eq!(s("knit_wool.png"), "Knit wool");
        assert_eq!(s("/tmp/red-glove-final-v2.png"), "Red glove");
        assert_eq!(s("ChatGPT Image Apr 3, 2025, 10_15_32 AM.png"), "My hand 3");
        assert_eq!(s("chatgpt_image_1.png"), "My hand 3");
        assert_eq!(s("supercalifragilistic.png"), "My hand 3");
        assert_eq!(s("___.png"), "My hand 3");
        assert_eq!(s("élan vital.png"), "Élan vital");
        assert_eq!(s("robot"), "Robot");
    }
}
