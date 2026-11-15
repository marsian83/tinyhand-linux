use super::{display::alpha_rects, draw::upscale};

const OPAQUE: u32 = 0xff00_0000;

#[test]
fn shapes_cover_exactly_the_opaque_pixels() {
    // A 4×4 buffer: a 2×2 block, then a lone pixel on the last row.
    let (w, h) = (4, 4);
    let mut px = vec![0u32; w * h];
    for (x, y) in [(1, 0), (2, 0), (1, 1), (2, 1), (0, 3)] {
        px[y * w + x] = OPAQUE;
    }
    px[2 * w + 3] = 0x7f00_0000; // half transparent: not part of the shape
    let rects = alpha_rects(&px, w, h);
    let covered = |x: i16, y: i16| {
        rects.iter().any(|r| x >= r.x && y >= r.y && x < r.x + r.width as i16 && y < r.y + r.height as i16)
    };
    for y in 0..h {
        for x in 0..w {
            assert_eq!(covered(x as i16, y as i16), px[y * w + x] >> 24 > 127, "({x}, {y})");
        }
    }
    assert_eq!(rects.len(), 2, "identical rows merge into one band: {rects:?}");
}

#[test]
fn upscaling_keeps_flat_colour_and_fills_the_target() {
    let src = vec![0x8040_2010u32; 4 * 3];
    let mut dst = vec![0u32; 16 * 12];
    upscale(&src, 4, 3, &mut dst, 16, 12);
    // Inside, a flat colour stays flat; at the edges it fades towards transparent.
    assert_eq!(dst[6 * 16 + 8], 0x8040_2010);
    assert!(dst.iter().all(|&p| p >> 24 <= 0x80));
    assert!(dst[0] >> 24 > 0, "corners still get some of the colour");
}
