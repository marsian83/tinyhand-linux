//! Ink: the strokes drawn by dragging while the pointer is on.
//!
//! Port of the look of `Ink.swift`: one stroke per drag, round caps and joins, smoothed
//! with quadratic curves through the midpoints of successive points, and a soft black
//! drop shadow (opacity 0.18, blur 1.5, offset +1 right / +1.5 down, logical px).
//!
//! Nothing here allocates a full-screen buffer. A stroke is a sparse set of square tiles
//! (each shown by the display side as its own small surface), and every tile belongs to
//! exactly one stroke so strokes can fade individually. Each tile stores two 8-bit
//! coverage planes (ink and shadow), interleaved per pixel. Pieces of the stroke are
//! rasterised incrementally as anti-aliased capsules (signed distance to a segment) and
//! merged with `max`, so overlapping pieces of one stroke never darken each other.
//!
//! Coordinates: logical px, y down. Internally everything is done in device px on a
//! global grid: device pixel `g` covers logical `[g / scale, (g + 1) / scale)`, and tile
//! `t` holds device pixels `[t * tile_px, (t + 1) * tile_px)`.

use std::collections::HashMap;

/// Points closer than this (logical px) to the previous point are skipped.
const MIN_STEP: f32 = 1.5;
/// Shadow colour is black at this opacity.
const SHADOW_OPACITY: f32 = 0.18;
/// Half-width of the shadow's soft edge, logical px.
const SHADOW_BLUR: f32 = 1.5;
/// Shadow offset, logical px (right, down).
const SHADOW_OFFSET: (f32, f32) = (1.0, 1.5);
/// Maximum distance between a curve and its flattened polyline, device px.
const FLATTEN_TOLERANCE: f32 = 0.15;

pub type Key = (i32, i32);
type Pt = (f32, f32);

/// Coverage of one tile: per device pixel `[ink, shadow]`, row-major, `tile_px²` long.
struct Tile {
    cov: Box<[[u8; 2]]>,
}

/// Per-alpha lookup tables for `Stroke::render_tile_with`.
pub struct Palette {
    ink: [u32; 256],
    ink_a: [u32; 256],
    shade: [u32; 256],
    k: u64,
    visible: bool,
}

/// One ink stroke (one drag), stored as sparse tiles of coverage.
pub struct Stroke {
    rgb: [u8; 3],
    scale: f32,
    tile: i32,
    tile_px: usize,
    /// Half the line width, device px.
    radius: f32,
    /// Shadow soft-edge half-width, device px.
    blur: f32,
    /// Shadow offset, device px.
    offset: Pt,
    /// Longest line piece rasterised at once, device px (keeps bounding boxes tight).
    max_piece: f32,
    tiles: HashMap<Key, Tile>,
    /// Last accepted input point (logical); `None` before `begin` or after `end`.
    last: Option<Pt>,
    /// Where the rasterised path currently ends (logical): the last midpoint.
    cursor: Pt,
    /// Whether any point after the first has been accepted.
    moved: bool,
}

impl Stroke {
    /// Create an empty stroke.
    ///
    /// `colour` is `0xRRGGBB`; `width` is the line width in logical px; `scale` is device
    /// px per logical px (e.g. 1.6); `tile` is the tile side in logical px, chosen by the
    /// caller so that `tile * scale` is a whole number.
    pub fn new(colour: u32, width: f32, scale: f32, tile: i32) -> Stroke {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let tile = tile.max(1);
        let tile_px = ((tile as f32 * scale).round() as usize).max(1);
        let radius = width.max(0.0) * scale / 2.0;
        let blur = SHADOW_BLUR * scale;
        Stroke {
            rgb: [(colour >> 16) as u8, (colour >> 8) as u8, colour as u8],
            scale,
            tile,
            tile_px,
            radius,
            blur,
            offset: (SHADOW_OFFSET.0 * scale, SHADOW_OFFSET.1 * scale),
            max_piece: (2.0 * (radius + blur)).max(8.0),
            tiles: HashMap::new(),
            last: None,
            cursor: (0.0, 0.0),
            moved: false,
        }
    }

    /// Start the stroke at `p` (logical px, y down) and draw a dot there (the stroke's
    /// round start cap). Returns the tiles whose pixels changed.
    pub fn begin(&mut self, p: Pt) -> Vec<Key> {
        self.last = Some(p);
        self.cursor = p;
        self.moved = false;
        let mut dirty = Vec::new();
        let d = self.device(p);
        self.capsule(d, d, &mut dirty);
        dirty
    }

    /// Add a point while dragging (logical px). Rasterises the new curve piece, from the
    /// previous midpoint to the new midpoint with the previous point as control. Points
    /// within 1.5 px of the previous one are ignored. Returns the tiles whose pixels
    /// changed (may be empty).
    pub fn extend(&mut self, p: Pt) -> Vec<Key> {
        let mut dirty = Vec::new();
        let Some(last) = self.last else {
            return dirty;
        };
        if (p.0 - last.0).hypot(p.1 - last.1) < MIN_STEP {
            return dirty;
        }
        let mid = ((last.0 + p.0) / 2.0, (last.1 + p.1) / 2.0);
        let (a, c, b) = (self.device(self.cursor), self.device(last), self.device(mid));
        self.quad(a, c, b, &mut dirty);
        self.cursor = mid;
        self.last = Some(p);
        self.moved = true;
        dirty
    }

    /// Finish the stroke: draws the final straight piece from the last midpoint to the
    /// last point. Further `extend` calls are ignored. Returns the tiles whose pixels
    /// changed.
    pub fn end(&mut self) -> Vec<Key> {
        let mut dirty = Vec::new();
        if let Some(last) = self.last.take() {
            if self.moved {
                let (a, b) = (self.device(self.cursor), self.device(last));
                self.line(a, b, &mut dirty);
            }
        }
        dirty
    }

    /// Every tile that has any ink or shadow, sorted.
    pub fn tiles(&self) -> Vec<Key> {
        let mut keys: Vec<Key> = self.tiles.keys().copied().collect();
        keys.sort_unstable();
        keys
    }

    /// Tile buffer side in device pixels (`tile * scale`, rounded).
    pub fn tile_px(&self) -> usize {
        self.tile_px
    }

    /// Logical position of a tile's top-left corner (`key * tile`).
    pub fn tile_origin(&self, key: Key) -> (i32, i32) {
        (key.0 * self.tile, key.1 * self.tile)
    }

    /// Lookup tables for rendering this stroke's tiles at `alpha` (0..=1, for fading).
    /// Build it once per frame and reuse it for every tile.
    pub fn palette(&self, alpha: f32) -> Palette {
        let alpha = if alpha.is_nan() { 0.0 } else { alpha.clamp(0.0, 1.0) };
        // Ink at coverage i (no shadow): premultiplied colour and alpha.
        let mut ink = [0u32; 256];
        let mut ink_a = [0u32; 256];
        for (i, (px, pa)) in ink.iter_mut().zip(ink_a.iter_mut()).enumerate() {
            let k = i as f32 / 255.0 * alpha;
            let ch = |c: u8| (c as f32 * k).round() as u32;
            *pa = (255.0 * k).round() as u32;
            *px = (*pa << 24) | (ch(self.rgb[0]) << 16) | (ch(self.rgb[1]) << 8) | ch(self.rgb[2]);
        }
        // Shadow alpha in 0..=46 (0.18 × 255) per shadow coverage.
        let mut shade = [0u32; 256];
        for (s, v) in shade.iter_mut().enumerate() {
            *v = (s as f32 * SHADOW_OPACITY).round() as u32;
        }
        // Final alpha = alpha · (ink·255 + shadow·(255 − ink)) / 255; k = alpha · 2^24 / 255.
        let k = (alpha as f64 * (1u64 << 24) as f64 / 255.0).round() as u64;
        Palette { ink, ink_a, shade, k, visible: alpha > 0.0 }
    }

    /// Render a tile as premultiplied ARGB (`0xAARRGGBB`), row-major `tile_px × tile_px`,
    /// fully overwriting `out`: the shadow (black, 0.18 × coverage) with the ink over it,
    /// everything multiplied by `alpha` (0..=1, for fading). A tile without ink renders
    /// transparent.
    ///
    /// # Panics
    /// If `out` is shorter than `tile_px²`.
    pub fn render_tile(&self, key: Key, alpha: f32, out: &mut [u32]) {
        self.render_tile_with(key, &self.palette(alpha), out);
    }

    /// `render_tile` with a palette from `palette()`.
    pub fn render_tile_with(&self, key: Key, pal: &Palette, out: &mut [u32]) {
        let n = self.tile_px * self.tile_px;
        let out = &mut out[..n];
        let Some(tile) = self.tiles.get(&key).filter(|_| pal.visible) else {
            out.fill(0);
            return;
        };
        for (o, &[ci, cs]) in out.iter_mut().zip(tile.cov.iter()) {
            *o = if cs == 0 || ci == 255 {
                pal.ink[ci as usize]
            } else {
                let ci32 = ci as u32;
                let num = (ci32 * 255 + pal.shade[cs as usize] * (255 - ci32)) as u64;
                let a = (((num * pal.k) + (1 << 23)) >> 24) as u32;
                let a = a.max(pal.ink_a[ci as usize]).min(255);
                (pal.ink[ci as usize] & 0x00ff_ffff) | (a << 24)
            };
        }
    }

    fn device(&self, p: Pt) -> Pt {
        (p.0 * self.scale, p.1 * self.scale)
    }

    /// Rasterise a quadratic Bézier (device px), flattened into short line pieces.
    fn quad(&mut self, a: Pt, c: Pt, b: Pt, dirty: &mut Vec<Key>) {
        // The second derivative is 2(a − 2c + b); a chord over a parameter step h deviates
        // by at most |a − 2c + b|·h²/4 from the curve.
        let dd = (a.0 - 2.0 * c.0 + b.0).hypot(a.1 - 2.0 * c.1 + b.1);
        let n = ((dd / (4.0 * FLATTEN_TOLERANCE)).sqrt().ceil() as usize).clamp(1, 64);
        let mut prev = a;
        for i in 1..=n {
            let t = i as f32 / n as f32;
            let u = 1.0 - t;
            let p = (u * u * a.0 + 2.0 * u * t * c.0 + t * t * b.0, u * u * a.1 + 2.0 * u * t * c.1 + t * t * b.1);
            self.line(prev, p, dirty);
            prev = p;
        }
    }

    /// Rasterise a straight line (device px), split so each capsule's box stays tight.
    fn line(&mut self, a: Pt, b: Pt, dirty: &mut Vec<Key>) {
        let len = (b.0 - a.0).hypot(b.1 - a.1);
        let n = ((len / self.max_piece).ceil() as usize).max(1);
        let mut prev = a;
        for i in 1..=n {
            let t = i as f32 / n as f32;
            let p = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
            self.capsule(prev, p, dirty);
            prev = p;
        }
    }

    /// Rasterise one round-capped segment a→b (device px) and its shadow into the tiles,
    /// visiting only pixels in its bounding box. Tiles are created on first non-zero
    /// coverage; tiles whose coverage grew are added to `dirty`.
    fn capsule(&mut self, a: Pt, b: Pt, dirty: &mut Vec<Key>) {
        let r = self.radius;
        let ink_reach = r + 0.5;
        let shadow_reach = r + self.blur;
        let (ox, oy) = self.offset;
        let min_x = (a.0.min(b.0) - ink_reach).min(a.0.min(b.0) + ox - shadow_reach);
        let max_x = (a.0.max(b.0) + ink_reach).max(a.0.max(b.0) + ox + shadow_reach);
        let min_y = (a.1.min(b.1) - ink_reach).min(a.1.min(b.1) + oy - shadow_reach);
        let max_y = (a.1.max(b.1) + ink_reach).max(a.1.max(b.1) + oy + shadow_reach);
        if !(min_x.is_finite() && max_x.is_finite() && min_y.is_finite() && max_y.is_finite()) {
            return;
        }
        // Pixel g's centre is g + 0.5.
        let gx0 = (min_x - 0.5).ceil() as i32;
        let gx1 = (max_x - 0.5).floor() as i32;
        let gy0 = (min_y - 0.5).ceil() as i32;
        let gy1 = (max_y - 0.5).floor() as i32;
        if gx0 > gx1 || gy0 > gy1 {
            return;
        }

        let (abx, aby) = (b.0 - a.0, b.1 - a.1);
        let len2 = abx * abx + aby * aby;
        let inv = if len2 > 1e-12 { 1.0 / len2 } else { 0.0 };
        // Shadow distance at q is the ink distance at q − offset; the projection
        // parameter shifts by a constant.
        let dt_shadow = (ox * abx + oy * aby) * inv;
        let ink_reach2 = ink_reach * ink_reach;
        let shadow_reach2 = shadow_reach * shadow_reach;
        let inv_soft = 1.0 / (2.0 * self.blur);

        let tp = self.tile_px as i32;
        let n = self.tile_px * self.tile_px;
        let tiles = &mut self.tiles;
        for ty in gy0.div_euclid(tp)..=gy1.div_euclid(tp) {
            for tx in gx0.div_euclid(tp)..=gx1.div_euclid(tp) {
                let key = (tx, ty);
                let (bx, by) = (tx * tp, ty * tp);
                let (x0, x1) = (gx0.max(bx), gx1.min(bx + tp - 1));
                let (y0, y1) = (gy0.max(by), gy1.min(by + tp - 1));
                // Looked up (or created) on the first pixel with coverage.
                let mut slot: Option<&mut Tile> = None;
                let mut changed = false;
                for gy in y0..=y1 {
                    let qy = gy as f32 + 0.5 - a.1;
                    let row = ((gy - by) as usize) * self.tile_px;
                    for gx in x0..=x1 {
                        let qx = gx as f32 + 0.5 - a.0;
                        let t = (qx * abx + qy * aby) * inv;

                        let tc = t.clamp(0.0, 1.0);
                        let (dx, dy) = (qx - tc * abx, qy - tc * aby);
                        let d2 = dx * dx + dy * dy;
                        let ci =
                            if d2 < ink_reach2 { ((ink_reach - d2.sqrt()).min(1.0) * 255.0 + 0.5) as u8 } else { 0 };

                        let ts = (t - dt_shadow).clamp(0.0, 1.0);
                        let (sx, sy) = (qx - ox - ts * abx, qy - oy - ts * aby);
                        let s2 = sx * sx + sy * sy;
                        let cs = if s2 < shadow_reach2 {
                            let x = ((shadow_reach - s2.sqrt()) * inv_soft).min(1.0);
                            (x * x * (3.0 - 2.0 * x) * 255.0 + 0.5) as u8
                        } else {
                            0
                        };

                        if ci == 0 && cs == 0 {
                            continue;
                        }
                        if slot.is_none() {
                            slot = Some(
                                tiles.entry(key).or_insert_with(|| Tile { cov: vec![[0u8; 2]; n].into_boxed_slice() }),
                            );
                        }
                        let Some(tile) = slot.as_deref_mut() else {
                            continue;
                        };
                        let cell = &mut tile.cov[row + (gx - bx) as usize];
                        if ci > cell[0] {
                            cell[0] = ci;
                            changed = true;
                        }
                        if cs > cell[1] {
                            cell[1] = cs;
                            changed = true;
                        }
                    }
                }
                if changed && !dirty.contains(&key) {
                    dirty.push(key);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCALE: f32 = 1.6;
    const TILE: i32 = 130;

    fn ink_at(s: &Stroke, p: Pt) -> (u8, u8) {
        let tp = s.tile_px as i32;
        let (gx, gy) = ((p.0 * s.scale).floor() as i32, (p.1 * s.scale).floor() as i32);
        let key = (gx.div_euclid(tp), gy.div_euclid(tp));
        match s.tiles.get(&key) {
            Some(t) => {
                let i = gy.rem_euclid(tp) as usize * s.tile_px + gx.rem_euclid(tp) as usize;
                (t.cov[i][0], t.cov[i][1])
            }
            None => (0, 0),
        }
    }

    fn wavy(s: &mut Stroke) -> Vec<Key> {
        let mut dirty = s.begin((100.0, 200.0));
        for i in 1..=200 {
            let x = 100.0 + i as f32 * 3.0;
            let y = 200.0 + (i as f32 * 0.08).sin() * 60.0;
            dirty.extend(s.extend((x, y)));
        }
        dirty.extend(s.end());
        dirty
    }

    #[test]
    fn tile_geometry() {
        let s = Stroke::new(0xff0000, 6.2, SCALE, TILE);
        assert_eq!(s.tile_px(), 208);
        assert_eq!(s.tile_origin((2, -1)), (260, -130));
    }

    #[test]
    fn dot_from_click() {
        let mut s = Stroke::new(0x8854f2, 6.2, SCALE, TILE);
        let d = s.begin((50.0, 60.0));
        assert_eq!(d, vec![(0, 0)]);
        assert!(s.end().is_empty());
        assert_eq!(s.tiles(), vec![(0, 0)]);
        assert_eq!(ink_at(&s, (50.0, 60.0)).0, 255);
        assert_eq!(ink_at(&s, (50.0, 64.0)).0, 0);
        // Shadow sits down-right of the dot.
        assert_eq!(ink_at(&s, (51.0, 61.5)).1, 255);
        assert_eq!(ink_at(&s, (51.0, 56.0)).1, 0);
        assert!(ink_at(&s, (51.0, 64.0)).1 > 150);
    }

    #[test]
    fn dot_near_origin_uses_negative_tiles() {
        let mut s = Stroke::new(0x000000, 6.2, SCALE, TILE);
        let d = s.begin((1.0, 1.0));
        assert!(d.contains(&(-1, -1)) && d.contains(&(0, 0)));
        assert_eq!(s.tiles(), vec![(-1, -1), (-1, 0), (0, -1), (0, 0)]);
    }

    #[test]
    fn tiny_moves_are_skipped() {
        let mut s = Stroke::new(0, 6.2, SCALE, TILE);
        s.begin((10.0, 10.0));
        assert!(s.extend((11.0, 10.5)).is_empty());
        assert!(!s.moved);
        assert!(s.end().is_empty());
    }

    #[test]
    fn extend_without_begin_is_ignored() {
        let mut s = Stroke::new(0, 6.2, SCALE, TILE);
        assert!(s.extend((10.0, 10.0)).is_empty());
        assert!(s.end().is_empty());
        assert!(s.tiles().is_empty());
    }

    #[test]
    fn stroke_is_covered_and_dirty_tiles_reported() {
        let mut s = Stroke::new(0x00ff00, 6.2, SCALE, TILE);
        let dirty = wavy(&mut s);
        let tiles = s.tiles();
        // Every tile with ink was reported dirty at some point, and vice versa.
        let mut d = dirty.clone();
        d.sort_unstable();
        d.dedup();
        assert_eq!(d, tiles);
        // Tiles are disjoint by construction (one coverage buffer per key). The smoothed
        // path passes through every midpoint of successive input points: all of them
        // must be solid ink, including where the stroke crosses tile seams.
        let pt = |i: i32| (100.0 + i as f32 * 3.0, 200.0 + (i as f32 * 0.08).sin() * 60.0);
        for i in 0..200 {
            let (a, b) = (pt(i), pt(i + 1));
            let m = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
            assert_eq!(ink_at(&s, m).0, 255, "gap at {m:?}");
        }
        // The last point itself is reached by end().
        let last = (700.0, 200.0 + (200.0f32 * 0.08).sin() * 60.0);
        assert_eq!(ink_at(&s, last).0, 255);
        // Far away: nothing.
        assert_eq!(ink_at(&s, (100.0, 400.0)), (0, 0));
    }

    #[test]
    fn every_tile_has_coverage() {
        let mut s = Stroke::new(0x00ff00, 6.2, SCALE, TILE);
        wavy(&mut s);
        for key in s.tiles() {
            let t = &s.tiles[&key];
            assert!(t.cov.iter().any(|c| c[0] > 0 || c[1] > 0), "empty tile {key:?}");
        }
    }

    #[test]
    fn redraw_over_same_place_is_not_dirty() {
        let mut s = Stroke::new(0, 6.2, SCALE, TILE);
        s.begin((10.0, 10.0));
        s.extend((40.0, 10.0));
        s.extend((70.0, 10.0));
        s.end();
        // A new stroke over the same pixels changes nothing.
        s.begin((40.0, 10.0));
        assert!(s.extend((55.0, 10.0)).is_empty());
    }

    #[test]
    fn render_is_premultiplied_and_fades() {
        let mut s = Stroke::new(0x8854f2, 6.2, SCALE, TILE);
        wavy(&mut s);
        let n = s.tile_px() * s.tile_px();
        let mut full = vec![0u32; n];
        let mut half = vec![0u32; n];
        let mut solid = false;
        for key in s.tiles() {
            s.render_tile(key, 1.0, &mut full);
            s.render_tile(key, 0.5, &mut half);
            for (&f, &h) in full.iter().zip(&half) {
                for px in [f, h] {
                    let a = px >> 24;
                    assert!((px >> 16 & 0xff) <= a && (px >> 8 & 0xff) <= a && (px & 0xff) <= a);
                }
                let (fa, ha) = ((f >> 24) as i32, (h >> 24) as i32);
                assert!((fa / 2 - ha).abs() <= 1, "{fa} vs {ha}");
            }
            solid |= full.contains(&0xff88_54f2);
        }
        assert!(solid, "no fully inked pixel");
        s.render_tile((99, 99), 1.0, &mut full);
        assert!(full.iter().all(|&p| p == 0));
        s.render_tile(s.tiles()[0], 0.0, &mut full);
        assert!(full.iter().all(|&p| p == 0));
    }

    #[test]
    fn shadow_only_pixel_has_shadow_alpha() {
        let mut s = Stroke::new(0xffffff, 6.2, SCALE, TILE);
        s.begin((50.0, 50.0));
        let mut out = vec![0u32; s.tile_px() * s.tile_px()];
        s.render_tile((0, 0), 1.0, &mut out);
        // Just below the dot's ink edge, inside the shadow's core.
        let p = (51.0, 53.7);
        let (gx, gy) = ((p.0 * SCALE) as usize, (p.1 * SCALE) as usize);
        let (ci, cs) = ink_at(&s, p);
        assert_eq!(ci, 0);
        assert!(cs > 150, "{cs}");
        let px = out[gy * s.tile_px() + gx];
        assert_eq!(px & 0x00ff_ffff, 0);
        assert!((40..=46).contains(&(px >> 24)), "{:x}", px);
    }
}
