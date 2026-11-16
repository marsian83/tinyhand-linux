//! Hand images: loading, measuring (port of `HandAsset.measure` in PointerStyle.swift)
//! and pre-scaling into a premultiplied ARGB sprite at the exact pixel size we draw.

use std::{
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
};

/// Where the parts of a hand image are, as fractions of the image (y from the top).
#[derive(Clone, Copy, Debug)]
pub struct Meta {
    pub aspect: f32,
    pub hand_top: f32,
    pub tip: f32,
    pub wrist: f32,
    pub rod_end: f32,
    pub tip_x: f32,
    pub rod_x: f32,
    pub hand_min_x: f32,
    pub hand_max_x: f32,
    /// Rod core (below the cuff) x-extent.
    pub rod_min_x: f32,
    pub rod_max_x: f32,
    /// The cuff: rows from the wrist to `cuff_end`, wider than the rod core.
    pub cuff_end: f32,
    pub cuff_min_x: f32,
    pub cuff_max_x: f32,
}

pub struct Image {
    pub w: usize,
    pub h: usize,
    /// Straight (non-premultiplied) RGBA.
    pub rgba: Vec<u8>,
}

pub fn load_png(path: &Path) -> Result<Image, String> {
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut dec = png::Decoder::new(BufReader::new(file));
    dec.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = dec.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("png too large")?];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let (w, h) = (info.width as usize, info.height as usize);
    let rgba = match info.color_type {
        png::ColorType::Rgba => {
            buf.truncate(w * h * 4);
            buf
        }
        png::ColorType::Rgb => {
            buf[..w * h * 3].as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect()
        }
        png::ColorType::GrayscaleAlpha => {
            buf[..w * h * 2].as_chunks::<2>().0.iter().flat_map(|p| [p[0], p[0], p[0], p[1]]).collect()
        }
        png::ColorType::Grayscale => buf[..w * h].iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return Err("unexpected indexed png after expand".into()),
    };
    Ok(Image { w, h, rgba })
}

/// Same rules as the macOS app: find fingertip, fist, wrist and the rod's handle end.
pub fn measure(img: &Image) -> Option<Meta> {
    let (w, h) = (img.w, img.h);
    let opaque = |x: usize, y: usize| img.rgba[(y * w + x) * 4 + 3] > 128;
    let counts: Vec<usize> = (0..h).map(|y| (0..w).filter(|&x| opaque(x, y)).count()).collect();
    let center = |y: usize| -> f32 {
        let mut lo = None;
        let mut hi = 0;
        for x in 0..w {
            if opaque(x, y) {
                lo.get_or_insert(x);
                hi = x;
            }
        }
        lo.map_or(0.5, |lo| (lo + hi) as f32 / 2.0 / w as f32)
    };
    let x_extent = |rows: std::ops::Range<usize>| -> (f32, f32) {
        let (mut min_x, mut max_x) = (w, 0);
        for y in rows {
            for x in 0..w {
                if opaque(x, y) {
                    min_x = min_x.min(x);
                    max_x = max_x.max(x);
                }
            }
        }
        if min_x > max_x {
            return (0.0, 1.0);
        }
        (min_x as f32 / w as f32, (max_x + 1) as f32 / w as f32)
    };

    let top = counts.iter().position(|&c| c > 0)?;
    let search_end = h.min(top + (0.45 * h as f64) as usize);
    let max_w = *counts[top..search_end].iter().max()?;
    if max_w == 0 {
        return None;
    }
    let fist = (top..h).find(|&y| counts[y] as f64 > 0.6 * max_w as f64)?;
    let wrist = (fist..h).find(|&y| (counts[y] as f64) < 0.35 * max_w as f64)?;
    let rod_w = counts[(h - 1).min(wrist + h / 50)];
    let last_row = counts.iter().rposition(|&c| c > 0).unwrap_or(h - 1);
    let from = (h - 1).min(wrist + h / 33);
    let rod_end = (from..h).find(|&y| counts[y] as f64 > 1.8 * rod_w as f64).unwrap_or(last_row);

    let (hand_min_x, hand_max_x) = x_extent(top..wrist);
    let (rod_min_x, rod_max_x) = x_extent(from.min(rod_end)..rod_end);
    let (cuff_min_x, cuff_max_x) = x_extent(wrist..from.min(rod_end));
    let fh = h as f32;
    Some(Meta {
        aspect: w as f32 / fh,
        hand_top: (top as f32 - 0.004 * fh).max(0.0) / fh,
        tip: top as f32 / fh,
        wrist: wrist as f32 / fh,
        rod_end: rod_end as f32 / fh,
        tip_x: center((h - 1).min(top + h / 200)),
        rod_x: center((wrist + rod_end) / 2),
        hand_min_x,
        hand_max_x,
        rod_min_x,
        rod_max_x,
        cuff_end: from.min(rod_end) as f32 / fh,
        cuff_min_x,
        cuff_max_x,
    })
}

/// The used part of a hand image (rows hand_top..rod_end), resized so the hand part is
/// `hand_px` pixels tall, stored as premultiplied 0xAARRGGBB.
pub struct Sprite {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u32>,
    pub meta: Meta,
    /// Sprite pixels per source image pixel.
    pub k: f32,
    /// Source image height, to turn fractions into sprite rows.
    pub src_h: f32,
    /// Source-image fraction where the sprite's first row starts.
    pub crop_top: f32,
    pub hand_px: u32,
    pub shadow: Option<Shadow>,
}

impl Sprite {
    pub fn build(img: &Image, meta: Meta, hand_px: u32) -> Sprite {
        let src_hand = (meta.wrist - meta.hand_top) * img.h as f32;
        let k = (hand_px as f32 / src_hand).min(1.0);
        let y0 = (meta.hand_top * img.h as f32).floor() as usize;
        let y1 = ((meta.rod_end * img.h as f32).ceil() as usize).min(img.h);

        // Filter one source row at a time (premultiplied f32), then the vertical pass
        // straight into the final pixels. Peak memory is the horizontal result
        // (out_w × crop_h), not several full-size float copies of the image.
        let crop_h = y1 - y0;
        let out_w = ((img.w as f32 * k).round() as usize).max(1);
        let out_h = ((crop_h as f32 * k).round() as usize).max(1);
        let htaps = taps(img.w, out_w);
        let mut row = vec![[0f32; 4]; img.w];
        let mut horiz = vec![[0f32; 4]; out_w * crop_h];
        for y in 0..crop_h {
            for (x, d) in row.iter_mut().enumerate() {
                let p = &img.rgba[((y + y0) * img.w + x) * 4..][..4];
                let a = p[3] as f32 / 255.0;
                *d = [p[0] as f32 * a, p[1] as f32 * a, p[2] as f32 * a, p[3] as f32];
            }
            for (o, (lo, ws)) in htaps.iter().enumerate() {
                let mut acc = [0f32; 4];
                for (j, &wt) in ws.iter().enumerate() {
                    let s = row[lo + j];
                    for c in 0..4 {
                        acc[c] += s[c] * wt;
                    }
                }
                horiz[y * out_w + o] = acc;
            }
        }
        drop(row);
        let vtaps = taps(crop_h, out_h);
        let mut px = vec![0u32; out_w * out_h];
        let q = |v: f32| v.round().clamp(0.0, 255.0) as u32;
        for (o, (lo, ws)) in vtaps.iter().enumerate() {
            for x in 0..out_w {
                let mut acc = [0f32; 4];
                for (j, &wt) in ws.iter().enumerate() {
                    let s = horiz[(lo + j) * out_w + x];
                    for c in 0..4 {
                        acc[c] += s[c] * wt;
                    }
                }
                let a = q(acc[3]);
                px[o * out_w + x] = a << 24 | q(acc[0]).min(a) << 16 | q(acc[1]).min(a) << 8 | q(acc[2]).min(a);
            }
        }
        Sprite {
            w: out_w,
            h: out_h,
            px,
            meta,
            k: out_h as f32 / crop_h as f32,
            src_h: img.h as f32,
            crop_top: y0 as f32 / img.h as f32,
            hand_px,
            shadow: None,
        }
    }

    /// Sprite row for a source-image fraction (from the top).
    #[inline]
    pub fn row(&self, frac: f32) -> f32 {
        (frac - self.crop_top) * self.src_h * self.k
    }
}

/// Triangle-filter taps for resampling `len` samples to `out` (good for downscaling):
/// for each output sample, the first input index and the normalised weights.
fn taps(len: usize, out: usize) -> Vec<(usize, Vec<f32>)> {
    let scale = out as f32 / len as f32;
    let support = if scale < 1.0 { 1.0 / scale } else { 1.0 };
    (0..out)
        .map(|o| {
            let c = (o as f32 + 0.5) / scale;
            let lo = ((c - support).floor().max(0.0)) as usize;
            let hi = ((c + support).ceil() as usize).min(len);
            let mut ws: Vec<f32> = (lo..hi).map(|i| (1.0 - ((i as f32 + 0.5 - c) / support).abs()).max(0.0)).collect();
            let sum: f32 = ws.iter().sum();
            if sum > 0.0 {
                ws.iter_mut().for_each(|v| *v /= sum);
            }
            (lo, ws)
        })
        .collect()
}

/// Find a hand image by name: `$TINYHAND_ASSETS`, next to the binary, the install
/// prefix, the XDG data dirs, then (debug builds) the checkout's assets/hands.
/// `custom-…` ids are the user's own hands (see `handmaker`).
pub fn find(name: &str) -> Option<PathBuf> {
    if name.starts_with("custom-") {
        if !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_') {
            return None;
        }
        let p = crate::handmaker::dir().join(format!("{name}.png"));
        return p.exists().then_some(p);
    }
    let file = format!("hand_{name}.png");
    let mut dirs = Vec::new();
    if let Ok(d) = std::env::var("TINYHAND_ASSETS") {
        dirs.push(PathBuf::from(d));
    }
    // A release tarball: hands in assets/ next to the binary.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            dirs.push(dir.join("assets"));
        }
    }
    // Installed layouts: <prefix>/bin/tinyhand → <prefix>/share/tinyhand/hands
    // (covers /usr, /usr/local and ~/.local), then the XDG data dirs.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(prefix) = exe.parent().and_then(|d| d.parent()) {
            dirs.push(prefix.join("share/tinyhand/hands"));
        }
    }
    if let Some(home) = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
    {
        dirs.push(home.join("tinyhand/hands"));
    }
    let data_dirs = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    for d in data_dirs.split(':').filter(|d| !d.is_empty()) {
        dirs.push(std::path::Path::new(d).join("tinyhand/hands"));
    }
    // Debug builds run from the checkout (`cargo run`, `cargo test`) also look in the
    // repo's assets/hands. Release builds don't, so a packaged binary carries no build
    // path (set TINYHAND_ASSETS to run a release build from the checkout).
    #[cfg(debug_assertions)]
    dirs.push(PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/hands")));
    dirs.into_iter().map(|d| d.join(&file)).find(|p| p.exists())
}

/// Pre-blurred shadow sources, built once per sprite so frames never blur anything.
/// Two blur levels (at rest and fully pressed); frames interpolate between them.
pub struct Shadow {
    /// Padding in sprite pixels around the sprite in these textures.
    pub pad: f32,
    /// Hand silhouette (rows above the wrist only), blurred, at half sprite resolution.
    pub hw: usize,
    pub hh: usize,
    pub hand: [Vec<u8>; 2],
    /// Rod cross-section (the rod is uniform along its length), blurred, full resolution.
    pub rod: [Vec<u8>; 2],
}

impl Shadow {
    #[inline]
    pub fn hand_at(&self, tx: f32, ty: f32, press: f32) -> f32 {
        let u = (tx + self.pad) * 0.5 - 0.5;
        let v = (ty + self.pad) * 0.5 - 0.5;
        // The padding guarantees the blurred silhouette is zero at the texture edges, so
        // anything outside the interior is simply 0 — no per-tap bounds checks needed.
        if u < 0.0 || v < 0.0 || u >= (self.hw - 1) as f32 || v >= (self.hh - 1) as f32 {
            return 0.0;
        }
        let (x0, y0) = (u as usize, v as usize);
        let (fx, fy) = (u - x0 as f32, v - y0 as f32);
        let i = y0 * self.hw + x0;
        let level = |t: &[u8]| {
            let (a, b, c, d) = (t[i] as f32, t[i + 1] as f32, t[i + self.hw] as f32, t[i + self.hw + 1] as f32);
            let top = a + (b - a) * fx;
            top + (c + (d - c) * fx - top) * fy
        };
        let a = level(&self.hand[0]);
        if press <= 0.003 {
            return a / 255.0;
        }
        let b = level(&self.hand[1]);
        (a + (b - a) * press) / 255.0
    }

    #[inline]
    pub fn rod_at(&self, tx: f32, press: f32) -> f32 {
        let n = self.rod[0].len();
        let u = tx + self.pad - 0.5;
        if u <= -1.0 || u >= n as f32 {
            return 0.0;
        }
        let uf = u.floor();
        let f = u - uf;
        let i = uf as isize;
        let level = |t: &Vec<u8>| {
            let get = |i: isize| if i < 0 || i >= n as isize { 0.0 } else { t[i as usize] as f32 };
            get(i) + (get(i + 1) - get(i)) * f
        };
        let a = level(&self.rod[0]);
        let b = if press > 0.003 { level(&self.rod[1]) } else { a };
        (a + (b - a) * press) / 255.0
    }
}

impl Sprite {
    /// Build the shadow textures for blur sigmas given in logical pixels (rest, pressed).
    /// `hand_pts_rest` is the hand's height in logical pixels when not pressed.
    pub fn build_shadow(&mut self, hand_pts_rest: f32, sigma_log: [f32; 2]) {
        let m = self.meta;
        let px_per_log = (self.row(m.wrist) - self.row(m.hand_top)) / hand_pts_rest;
        let sig = sigma_log.map(|s| (s * px_per_log).max(0.5));
        let pad = (3.0 * sig[0].max(sig[1])).ceil() as usize + 2;

        let wrist_row = (self.row(m.wrist).round().max(0.0) as usize).min(self.h);
        let hw = (self.w + 2 * pad).div_ceil(2);
        let hh = (wrist_row + 2 * pad).div_ceil(2);
        let mut base = vec![0f32; hw * hh];
        for y in 0..wrist_row {
            for x in 0..self.w {
                let a = (self.px[y * self.w + x] >> 24) as f32;
                base[((y + pad) / 2) * hw + (x + pad) / 2] += a * 0.25;
            }
        }
        let hand = sig.map(|s| blur_u8(&base, hw, hh, s * 0.5));

        let r0 = (self.row(m.cuff_end).round().max(0.0) as usize).min(self.h);
        let r1 = (self.row(m.rod_end).floor().max(0.0) as usize).min(self.h).max(r0 + 1).min(self.h);
        let n = self.w + 2 * pad;
        let mut prof = vec![0f32; n];
        if r1 > r0 {
            for x in 0..self.w {
                let sum: u32 = (r0..r1).map(|y| self.px[y * self.w + x] >> 24).sum();
                prof[x + pad] = sum as f32 / (r1 - r0) as f32;
            }
        }
        let rod = sig.map(|s| blur_u8(&prof, n, 1, s));
        self.shadow = Some(Shadow { pad: pad as f32, hw, hh, hand, rod });
    }
}

/// Gaussian blur (three box passes per axis) of an f32 plane in 0..255, quantised to u8.
fn blur_u8(src: &[f32], w: usize, h: usize, sigma: f32) -> Vec<u8> {
    let r = ((((12.0 * sigma * sigma / 3.0) + 1.0).sqrt() - 1.0) / 2.0).round() as usize;
    let mut a = src.to_vec();
    let mut b = vec![0f32; a.len()];
    if r > 0 {
        for _ in 0..3 {
            box_pass(&a, &mut b, w, h, r, true);
            if h > 1 {
                box_pass(&b, &mut a, w, h, r, false);
            } else {
                std::mem::swap(&mut a, &mut b);
            }
        }
    }
    a.iter().map(|v| v.round().clamp(0.0, 255.0) as u8).collect()
}

fn box_pass(src: &[f32], dst: &mut [f32], w: usize, h: usize, r: usize, horizontal: bool) {
    let norm = 1.0 / (2 * r + 1) as f32;
    if horizontal {
        for y in 0..h {
            let s = &src[y * w..(y + 1) * w];
            let d = &mut dst[y * w..(y + 1) * w];
            let mut acc: f32 = s[..r.min(w)].iter().sum();
            for i in 0..w {
                if i + r < w {
                    acc += s[i + r];
                }
                d[i] = acc * norm;
                if i >= r {
                    acc -= s[i - r];
                }
            }
        }
    } else {
        // Row-at-a-time running sums keep the vertical pass cache-friendly.
        let mut acc = vec![0f32; w];
        for y in 0..r.min(h) {
            for (a, v) in acc.iter_mut().zip(&src[y * w..(y + 1) * w]) {
                *a += v;
            }
        }
        for y in 0..h {
            if y + r < h {
                for (a, v) in acc.iter_mut().zip(&src[(y + r) * w..(y + r + 1) * w]) {
                    *a += v;
                }
            }
            for (d, a) in dst[y * w..(y + 1) * w].iter_mut().zip(&acc) {
                *d = a * norm;
            }
            if y >= r {
                for (a, v) in acc.iter_mut().zip(&src[(y - r) * w..(y - r + 1) * w]) {
                    *a -= v;
                }
            }
        }
    }
}

/// Resolve a hand id or a PNG path to a file.
pub fn resolve(name: &str) -> Result<PathBuf, String> {
    let p = PathBuf::from(name);
    if p.extension().is_some() && p.exists() {
        return Ok(p);
    }
    find(name).ok_or_else(|| format!("can't find hand '{name}' (try `tinyhand hands`, or set TINYHAND_ASSETS)"))
}
