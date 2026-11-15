//! CPU rasteriser for the hand on a stick.
//!
//! Nothing here ever touches a full-screen buffer:
//! - The arm is split into tiles: one for the hand (+ cuff) and short tiles along the
//!   rod, so a long diagonal rod doesn't drag in a huge, mostly-empty bounding box.
//!   Each tile is its own small subsurface. Tiles own disjoint slices of the arm
//!   (by distance along the rod), so overlapping tile rectangles never double-draw.
//! - The soft shadow is drawn at a quarter of the resolution into its own subsurface and
//!   the compositor scales it up on the GPU. It samples blurs precomputed in
//!   `hand::Shadow`, so no frame ever blurs anything.
//! - Texture sampling is fixed-point bilinear, touching only pixels inside the rotated
//!   rod/hand rectangles.

use crate::{hand::Sprite, pose::Pose};

/// A placement: logical rect on the output (y down) and the buffer size in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub bw: usize,
    pub bh: usize,
}

/// A slice of the arm, by distance along the rod from the grip: gy in [g0, g1).
#[derive(Clone, Copy, Debug)]
pub struct Tile {
    pub frame: Frame,
    pub g0: f32,
    pub g1: f32,
}

pub struct Plan {
    pub tiles: Vec<Tile>,
    pub shadow: Option<Frame>,
}

/// Rod length per tile, logical pixels.
const SEGMENT: f32 = 120.0;
/// Shadow buffer pixels are this many device pixels wide.
pub const SHADOW_STEP: f32 = 4.0;

/// f(x, y) = a + b·x + c·y over buffer pixel coordinates.
#[derive(Clone, Copy)]
struct Aff {
    a: f32,
    b: f32,
    c: f32,
}

impl Aff {
    #[inline]
    fn at(&self, x: f32, y: f32) -> f32 {
        self.a + self.b * x + self.c * y
    }
    fn scaled(&self, k: f32, offset: f32) -> Aff {
        Aff { a: self.a * k + offset, b: self.b * k, c: self.c * k }
    }
}

type Rect = [(f32, f32); 2];

/// The arm's rectangles in arm space (x across the image, y along the rod from the grip).
struct Rects {
    core: Rect,
    cuff: Rect,
    hand: Rect,
    cuff_gy: f32,
}

fn rects(p: &Pose, spr: &Sprite, margin: f32) -> Rects {
    let m = &spr.meta;
    let w = p.width_pts;
    let cuff_frac = ((m.cuff_end - m.wrist) / (m.rod_end - m.wrist)).clamp(0.0, 1.0);
    let cuff_gy = p.rod_len * (1.0 - cuff_frac);
    let x = |lo: f32, hi: f32| (lo * w - margin, hi * w + margin);
    let (c0, c1) = x(m.rod_min_x, m.rod_max_x);
    let (u0, u1) = x(m.cuff_min_x, m.cuff_max_x);
    let (h0, h1) = x(m.hand_min_x, m.hand_max_x);
    Rects {
        core: [(c0, 0.0), (c1, cuff_gy)],
        cuff: [(u0, cuff_gy), (u1, p.rod_len)],
        hand: [(h0, p.rod_len), (h1, p.rod_len + p.hand_pts + margin)],
        cuff_gy,
    }
}

/// Arm-space point → output logical coordinates, y down.
fn to_output(p: &Pose, spr: &Sprite, gx: f32, gy: f32, out_h: f32) -> (f32, f32) {
    let ax = spr.meta.rod_x * p.width_pts;
    let (s, c) = p.rotation.sin_cos();
    let x = p.grip.0 + c * (gx - ax) - s * gy;
    let y = p.grip.1 + s * (gx - ax) + c * gy;
    (x, out_h - y)
}

fn corners(r: &Rect) -> [(f32, f32); 4] {
    [(r[0].0, r[0].1), (r[1].0, r[0].1), (r[1].0, r[1].1), (r[0].0, r[1].1)]
}

fn expand(r: &Rect, d: f32) -> Rect {
    [(r[0].0 - d, r[0].1 - d), (r[1].0 + d, r[1].1 + d)]
}

fn clip_y(r: &Rect, g0: f32, g1: f32) -> Option<Rect> {
    let (y0, y1) = (r[0].1.max(g0), r[1].1.min(g1));
    (y0 < y1).then_some([(r[0].0, y0), (r[1].0, y1)])
}

/// Logical bounding box of arm-space rects, shifted, as a clipped frame.
#[allow(clippy::too_many_arguments)]
fn frame_of(
    p: &Pose,
    spr: &Sprite,
    rs: &[Rect],
    shift: (f32, f32),
    out_w: i32,
    out_h: i32,
    scale: f32,
    step: f32,
) -> Option<Frame> {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for r in rs {
        for (gx, gy) in corners(r) {
            let (x, y) = to_output(p, spr, gx, gy, out_h as f32);
            let (x, y) = (x + shift.0, y + shift.1);
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    // Snap to a grid where logical pixels land on whole buffer pixels, so the compositor
    // shows buffers 1:1 (no resampling) and neighbouring tiles butt together seamlessly.
    let u = pixel_unit(scale, step) as f32;
    let snap_lo = |v: f32| (v / u).floor() as i32 * u as i32;
    let snap_hi = |v: f32| (v / u).ceil() as i32 * u as i32;
    let x0 = snap_lo(x0).max(0);
    let y0 = snap_lo(y0).max(0);
    // Past the far edge we round outwards rather than clip, to keep the grid exact;
    // the compositor crops anything off the output.
    let x1 = snap_hi(x1.min(out_w as f32));
    let y1 = snap_hi(y1.min(out_h as f32));
    if x1 <= x0 || y1 <= y0 || x0 >= out_w || y0 >= out_h {
        return None;
    }
    let (w, h) = (x1 - x0, y1 - y0);
    Some(Frame {
        x: x0,
        y: y0,
        w,
        h,
        bw: ((w as f32 * scale / step).round() as usize).max(1),
        bh: ((h as f32 * scale / step).round() as usize).max(1),
    })
}

/// Smallest logical size that is a whole number of buffer pixels at this scale and step
/// (e.g. 5 at 1.6×, 2 at 1.5×, 1 at integer scales). Falls back to 1 for odd scales.
pub fn pixel_unit(scale: f32, step: f32) -> i32 {
    (1..=40)
        .find(|&u| {
            let v = u as f32 * scale / step;
            (v - v.round()).abs() < 1e-3
        })
        .unwrap_or(1)
}

fn shadow_shift(p: &Pose) -> (f32, f32) {
    // Pose offsets are y-up; frames are y-down.
    (p.shadow_off.0, -p.shadow_off.1)
}

/// Decide which tiles and shadow buffer this pose needs.
pub fn plan(p: &Pose, spr: &Sprite, out_w: i32, out_h: i32, scale: f32) -> Plan {
    let r = rects(p, spr, 2.0 / scale);
    let mut tiles = Vec::new();
    let mut push = |rs: &[Rect], g0: f32, g1: f32| {
        if let Some(frame) = frame_of(p, spr, rs, (0.0, 0.0), out_w, out_h, scale, 1.0) {
            tiles.push(Tile { frame, g0, g1 });
        }
    };
    push(&[r.cuff, r.hand], r.cuff_gy, f32::INFINITY);
    let mut b = r.cuff_gy;
    while b > 0.0 {
        let a = (b - SEGMENT).max(0.0);
        if let Some(seg) = clip_y(&r.core, a, b) {
            push(&[seg], if a <= 0.0 { f32::NEG_INFINITY } else { a }, b);
        }
        b = a;
    }

    let shadow = (p.shadow_opacity > 0.0).then(|| {
        let ms = 1.5 * p.shadow_radius; // 3σ with σ = radius / 2
        let rod = [(r.core[0].0.min(r.cuff[0].0) - ms, 0.0), (r.core[1].0.max(r.cuff[1].0) + ms, p.rod_len + ms)];
        frame_of(p, spr, &[expand(&r.hand, ms), rod], shadow_shift(p), out_w, out_h, scale, SHADOW_STEP)
    });
    Plan { tiles, shadow: shadow.flatten() }
}

/// Buffer pixel → arm space and texture coordinates, for one frame.
struct Map {
    gy: Aff,
    tx: Aff,
    rod_ty: Aff,
    hand_ty: Aff,
    kx: f32,
    ky: f32,
    fx: f32,
    fy: f32,
    shift: (f32, f32),
}

fn make_map(p: &Pose, spr: &Sprite, f: &Frame, shift: (f32, f32), out_h: f32) -> Map {
    let m = &spr.meta;
    // Exactly the compositor's buffer→logical mapping (buffer is stretched to w×h).
    let kx = f.w as f32 / f.bw as f32;
    let ky = f.h as f32 / f.bh as f32;
    let ax = m.rod_x * p.width_pts;
    let (s, c) = p.rotation.sin_cos();
    let dx = Aff { a: f.x as f32 - shift.0 - p.grip.0, b: kx, c: 0.0 };
    let dy = Aff { a: out_h - f.y as f32 + shift.1 - p.grip.1, b: 0.0, c: -ky };
    let gx = Aff { a: ax + c * dx.a + s * dy.a, b: c * dx.b + s * dy.b, c: c * dx.c + s * dy.c };
    let gy = Aff { a: -s * dx.a + c * dy.a, b: -s * dx.b + c * dy.b, c: -s * dx.c + c * dy.c };
    let rows = spr.src_h * spr.k;
    let rod_k = -(m.rod_end - m.wrist) / p.rod_len * rows;
    let hand_k = -(m.wrist - m.hand_top) / p.hand_pts * rows;
    Map {
        gy,
        tx: gx.scaled(spr.w as f32 / p.width_pts, 0.0),
        rod_ty: gy.scaled(rod_k, spr.row(m.rod_end)),
        hand_ty: gy.scaled(hand_k, spr.row(m.wrist) - hand_k * p.rod_len),
        kx,
        ky,
        fx: f.x as f32,
        fy: f.y as f32,
        shift,
    }
}

impl Map {
    fn poly(&self, p: &Pose, spr: &Sprite, r: &Rect, out_h: f32) -> [(f32, f32); 4] {
        corners(r).map(|(gx, gy)| {
            let (x, y) = to_output(p, spr, gx, gy, out_h);
            ((x + self.shift.0 - self.fx) / self.kx, (y + self.shift.1 - self.fy) / self.ky)
        })
    }
}

/// x-extent of a convex polygon within the horizontal band [y0, y1].
fn band_span(poly: &[(f32, f32); 4], y0: f32, y1: f32) -> Option<(f32, f32)> {
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for i in 0..4 {
        let (a, b) = (poly[i], poly[(i + 1) % 4]);
        if a.1 >= y0 && a.1 <= y1 {
            lo = lo.min(a.0);
            hi = hi.max(a.0);
        }
        for y in [y0, y1] {
            if (a.1 - y) * (b.1 - y) < 0.0 {
                let x = a.0 + (b.0 - a.0) * (y - a.1) / (b.1 - a.1);
                lo = lo.min(x);
                hi = hi.max(x);
            }
        }
    }
    (lo <= hi).then_some((lo, hi))
}

fn pixel_span(poly: &[(f32, f32); 4], y: usize, width: usize) -> Option<(usize, usize)> {
    let (lo, hi) = band_span(poly, y as f32, y as f32 + 1.0)?;
    let a = lo.floor().max(0.0) as usize;
    let b = (hi.ceil().max(0.0) as usize).min(width);
    (a < b).then_some((a, b))
}

#[inline(always)]
fn lerp(a: u32, b: u32, t: u32) -> u32 {
    let it = 256 - t;
    let rb = (((a & 0x00ff00ff) * it + (b & 0x00ff00ff) * t) >> 8) & 0x00ff00ff;
    let ag = (((a >> 8) & 0x00ff00ff) * it + ((b >> 8) & 0x00ff00ff) * t) & 0xff00ff00;
    rb | ag
}

/// Bilinear sample at 16.16 fixed-point texel coordinates (already offset by -0.5).
#[inline(always)]
fn sample_fx(s: &Sprite, fx: i32, fy: i32) -> u32 {
    let (x, y) = (fx >> 16, fy >> 16);
    let (tx, ty) = (((fx >> 8) & 255) as u32, ((fy >> 8) & 255) as u32);
    let w = s.w;
    if (x as u32) < (w - 1) as u32 && (y as u32) < (s.h - 1) as u32 {
        let i = y as usize * w + x as usize;
        let px = &s.px[i..i + w + 2];
        return lerp(lerp(px[0], px[1], tx), lerp(px[w], px[w + 1], tx), ty);
    }
    if x < -1 || y < -1 || x >= w as i32 || y >= s.h as i32 {
        return 0;
    }
    let get = |xx: i32, yy: i32| {
        if xx < 0 || yy < 0 || xx >= w as i32 || yy >= s.h as i32 {
            0
        } else {
            s.px[yy as usize * w + xx as usize]
        }
    };
    lerp(lerp(get(x, y), get(x + 1, y), tx), lerp(get(x, y + 1), get(x + 1, y + 1), tx), ty)
}

const FX: f32 = 65536.0;

/// Draw one tile into `out` (premultiplied ARGB, `bw`×`bh`, fully overwritten).
pub fn render_tile(p: &Pose, spr: &Sprite, t: &Tile, out_h: f32, scale: f32, out: &mut [u32]) {
    let f = &t.frame;
    out[..f.bw * f.bh].fill(0);
    let map = make_map(p, spr, f, (0.0, 0.0), out_h);
    let r = rects(p, spr, 2.0 / scale);
    let quads = [
        (r.core, map.rod_ty, f32::NEG_INFINITY, r.cuff_gy),
        (r.cuff, map.rod_ty, r.cuff_gy, p.rod_len),
        (r.hand, map.hand_ty, p.rod_len, f32::INFINITY),
    ];
    for (rect, ty, q0, q1) in quads {
        let (g0, g1) = (q0.max(t.g0), q1.min(t.g1));
        let Some(rect) = clip_y(&rect, g0, g1) else { continue };
        let poly = map.poly(p, spr, &rect, out_h);
        let (dfx, dfy, dgy) = ((map.tx.b * FX) as i32, (ty.b * FX) as i32, map.gy.b);
        for y in 0..f.bh {
            let Some((a, b)) = pixel_span(&poly, y, f.bw) else { continue };
            let (xc, yc) = (a as f32 + 0.5, y as f32 + 0.5);
            let mut fx = ((map.tx.at(xc, yc) - 0.5) * FX) as i32;
            let mut fy = ((ty.at(xc, yc) - 0.5) * FX) as i32;
            let mut gy = map.gy.at(xc, yc);
            for px in &mut out[y * f.bw + a..y * f.bw + b] {
                if gy >= g0 && gy < g1 {
                    *px = sample_fx(spr, fx, fy);
                }
                fx = fx.wrapping_add(dfx);
                fy = fy.wrapping_add(dfy);
                gy += dgy;
            }
        }
    }
}

/// Draw the shadow (black, premultiplied alpha) at `SHADOW_STEP` resolution.
pub fn render_shadow(p: &Pose, spr: &Sprite, f: &Frame, out_h: f32, scale: f32, out: &mut [u32]) {
    out[..f.bw * f.bh].fill(0);
    let Some(sh) = spr.shadow.as_ref() else { return };
    let map = make_map(p, spr, f, shadow_shift(p), out_h);
    let r = rects(p, spr, 2.0 / scale);
    let sigma = p.shadow_radius / 2.0;
    let ms = 3.0 * sigma;
    let rod = [(r.core[0].0.min(r.cuff[0].0) - ms, -ms), (r.core[1].0.max(r.cuff[1].0) + ms, p.rod_len + ms)];
    let polys = [map.poly(p, spr, &expand(&r.hand, ms), out_h), map.poly(p, spr, &rod, out_h)];
    let op = p.shadow_opacity;
    let end_k = 1.0 / (2.5 * sigma);
    for y in 0..f.bh {
        let spans: Vec<(usize, usize)> = polys.iter().filter_map(|poly| pixel_span(poly, y, f.bw)).collect();
        let (Some(a), Some(b)) = (spans.iter().map(|s| s.0).min(), spans.iter().map(|s| s.1).max()) else { continue };
        let (xc, yc) = (a as f32 + 0.5, y as f32 + 0.5);
        let mut tx = map.tx.at(xc, yc);
        let mut ty = map.hand_ty.at(xc, yc);
        let mut gy = map.gy.at(xc, yc);
        for px in &mut out[y * f.bw + a..y * f.bw + b] {
            let ah = sh.hand_at(tx, ty, p.press);
            let ar = if gy > -ms && gy < p.rod_len + ms {
                sh.rod_at(tx, p.press) * (0.5 + (p.rod_len - gy) * end_k).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let alpha = 1.0 - (1.0 - op * ah) * (1.0 - op * ar);
            let a8 = (alpha * 255.0 + 0.5) as u32;
            if a8 > 0 {
                *px = a8.min(255) << 24;
            }
            tx += map.tx.b;
            ty += map.hand_ty.b;
            gy += map.gy.b;
        }
    }
}

/// Blur sigmas (logical px) for the two precomputed shadow levels, from the output height.
pub fn shadow_sigmas(out_h: f32) -> [f32; 2] {
    [crate::pose::shadow_radius(out_h, 0.0) / 2.0, crate::pose::shadow_radius(out_h, 1.0) / 2.0]
}

/// The laser dot (port of `ArmView.aimLaser`): a red dot with a pale core and a red glow.
pub struct Laser {
    /// Centre, logical px, y down.
    pub at: (f32, f32),
    pub r: f32,
    pub glow: f32,
}

impl Laser {
    pub fn new(at: (f32, f32), out_h: f32, size: f32, press: f32) -> Laser {
        let r = (out_h * 0.0038 * size / 0.13).max(4.0) * (1.0 + 0.2 * press);
        Laser { at, r, glow: r * (2.2 + press) }
    }

    /// Same rounding as `Pose::key`: frames with equal keys look identical.
    pub fn key(&self) -> [i32; 3] {
        let k = |v: f32| (v * 100.0).round() as i32;
        [k(self.at.0), k(self.at.1), k(self.r)]
    }

    pub fn frame(&self, out_w: i32, out_h: i32, scale: f32) -> Option<Frame> {
        let ext = self.r + 1.5 * self.glow + 1.0;
        let u = pixel_unit(scale, 1.0) as f32;
        let x0 = ((self.at.0 - ext) / u).floor() as i32 * u as i32;
        let y0 = ((self.at.1 - ext) / u).floor() as i32 * u as i32;
        let x1 = ((self.at.0 + ext) / u).ceil() as i32 * u as i32;
        let y1 = ((self.at.1 + ext) / u).ceil() as i32 * u as i32;
        let (x0, y0) = (x0.max(0), y0.max(0));
        if x1 <= x0 || y1 <= y0 || x0 >= out_w || y0 >= out_h {
            return None;
        }
        let (w, h) = (x1 - x0, y1 - y0);
        Some(Frame {
            x: x0,
            y: y0,
            w,
            h,
            bw: (w as f32 * scale).round() as usize,
            bh: (h as f32 * scale).round() as usize,
        })
    }

    pub fn render(&self, f: &Frame, colour: u32, out: &mut [u32]) {
        let (kx, ky) = (f.w as f32 / f.bw as f32, f.h as f32 / f.bh as f32);
        let px_per = 1.0 / kx;
        let core = self.r * 0.45;
        let sigma = self.glow / 2.0;
        let (cr, cg, cb) = (((colour >> 16) & 255) as f32, ((colour >> 8) & 255) as f32, (colour & 255) as f32);
        let core_rgb = (255.0, 219.0, 209.0);
        for by in 0..f.bh {
            for bx in 0..f.bw {
                let x = f.x as f32 + (bx as f32 + 0.5) * kx - self.at.0;
                let y = f.y as f32 + (by as f32 + 0.5) * ky - self.at.1;
                let d = (x * x + y * y).sqrt();
                // Glow: the disc blurred by a Gaussian, approximated with a logistic edge.
                let glow = 1.0 / (1.0 + ((d - self.r) / (sigma * 0.6)).exp());
                let dot = ((self.r - d) * px_per + 0.5).clamp(0.0, 1.0);
                let cov_core = ((core - d) * px_per + 0.5).clamp(0.0, 1.0) * 0.95;
                // Composite glow, then the dot, then the core (premultiplied).
                let mut a = glow;
                let (mut r, mut g, mut b) = (cr * glow, cg * glow, cb * glow);
                a = dot + a * (1.0 - dot);
                r = cr * dot + r * (1.0 - dot);
                g = cg * dot + g * (1.0 - dot);
                b = cb * dot + b * (1.0 - dot);
                a = cov_core + a * (1.0 - cov_core);
                r = core_rgb.0 * cov_core + r * (1.0 - cov_core);
                g = core_rgb.1 * cov_core + g * (1.0 - cov_core);
                b = core_rgb.2 * cov_core + b * (1.0 - cov_core);
                let q = |v: f32| (v.round() as u32).min(255);
                let a8 = q(a * 255.0);
                out[by * f.bw + bx] = a8 << 24 | q(r).min(a8) << 16 | q(g).min(a8) << 8 | q(b).min(a8);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{hand, pose::Arm};

    fn sprite() -> Sprite {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/hands/hand_coral.png");
        let img = hand::load_png(&path).unwrap();
        let meta = hand::measure(&img).unwrap();
        let mut s = Sprite::build(&img, meta, 234);
        s.build_shadow(146.0, shadow_sigmas(1125.0));
        s
    }

    #[test]
    fn pixel_unit_lands_on_whole_pixels() {
        assert_eq!(pixel_unit(1.0, 1.0), 1);
        assert_eq!(pixel_unit(1.6, 1.0), 5);
        assert_eq!(pixel_unit(1.5, 1.0), 2);
        assert_eq!(pixel_unit(1.6, SHADOW_STEP), 5);
    }

    /// The fingertip must land on the cursor, as on macOS.
    #[test]
    fn fingertip_is_at_the_cursor() {
        let s = sprite();
        let (w, h, scale) = (1800, 1125, 1.6);
        for &(cx, cy) in &[(900.0f32, 500.0f32), (200.0, 900.0), (1600.0, 100.0)] {
            let mut arm = Arm::new(0.13, true);
            let p = arm.step((cx, h as f32 - cy), w as f32, h as f32, 0.0, &s.meta);
            let plan = plan(&p, &s, w, h, scale);
            // Find the topmost opaque pixel near the cursor across all tiles.
            let mut best: Option<(f32, f32)> = None;
            for t in &plan.tiles {
                let f = t.frame;
                let mut buf = vec![0u32; f.bw * f.bh];
                render_tile(&p, &s, t, h as f32, scale, &mut buf);
                for y in 0..f.bh {
                    for x in 0..f.bw {
                        if buf[y * f.bw + x] >> 24 > 128 {
                            let lx = f.x as f32 + (x as f32 + 0.5) / scale;
                            let ly = f.y as f32 + (y as f32 + 0.5) / scale;
                            let d = (lx - cx).hypot(ly - cy);
                            if best.is_none_or(|b| d < (b.0 - cx).hypot(b.1 - cy)) {
                                best = Some((lx, ly));
                            }
                        }
                    }
                }
            }
            let (bx, by) = best.unwrap();
            assert!((bx - cx).hypot(by - cy) < 6.0, "tip at {bx},{by} for cursor {cx},{cy}");
        }
    }

    #[test]
    fn tiles_never_draw_the_same_pixel_twice() {
        let s = sprite();
        let (w, h, scale) = (1800, 1125, 1.6);
        let mut arm = Arm::new(0.13, true);
        let p = arm.step((500.0, 900.0), w as f32, h as f32, 0.0, &s.meta);
        let plan = plan(&p, &s, w, h, scale);
        assert!(plan.tiles.len() > 2);
        let (cw, ch) = ((w as f32 * scale) as usize, (h as f32 * scale) as usize);
        let mut owner = vec![u8::MAX; cw * ch];
        for (i, t) in plan.tiles.iter().enumerate() {
            let f = t.frame;
            let mut buf = vec![0u32; f.bw * f.bh];
            render_tile(&p, &s, t, h as f32, scale, &mut buf);
            for y in 0..f.bh {
                for x in 0..f.bw {
                    if buf[y * f.bw + x] != 0 {
                        let gx = (f.x as f32 * scale) as usize + x;
                        let gy = (f.y as f32 * scale) as usize + y;
                        if gx < cw && gy < ch {
                            assert_eq!(
                                owner[gy * cw + gx],
                                u8::MAX,
                                "pixel {gx},{gy} drawn by tiles {} and {i}",
                                owner[gy * cw + gx]
                            );
                            owner[gy * cw + gx] = i as u8;
                        }
                    }
                }
            }
        }
    }
}
