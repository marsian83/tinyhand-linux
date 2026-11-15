//! `tinyhand bench`: offscreen rendering benchmark, and a contact sheet to check the
//! drawing by eye (`--out sheet.png`).

use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

use crate::{config::Config, hand, pose, raster};

/// Offscreen benchmark: load + build the sprite, then render a cursor sweeping the screen
/// at 120 Hz on a 2880×1800 @1.6 output (logical 1800×1125). Optionally writes a contact
/// sheet PNG (composited the way the compositor would) to check the drawing by eye.
pub fn run(cfg: &Config, out: Option<&Path>) -> Result<(), String> {
    let (w, h, scale) = (1800, 1125, 1.6f32);
    let path = hand::resolve(&cfg.hand)?;
    let t0 = Instant::now();
    let img = hand::load_png(&path)?;
    let t1 = Instant::now();
    let meta = hand::measure(&img).ok_or("can't measure")?;
    let t2 = Instant::now();
    let hand_px = (cfg.size * h as f32 * scale).round() as u32;
    let mut sprite = hand::Sprite::build(&img, meta, hand_px);
    let t3 = Instant::now();
    sprite.build_shadow(cfg.size * h as f32, raster::shadow_sigmas(h as f32));
    let t4 = Instant::now();
    drop(img);
    println!(
        "decode {:.1} ms, measure {:.1} ms, sprite {:.1} ms, shadow {:.1} ms -> sprite {}x{} ({} KB)",
        ms(t1 - t0),
        ms(t2 - t1),
        ms(t3 - t2),
        ms(t4 - t3),
        sprite.w,
        sprite.h,
        sprite.w * sprite.h * 4 / 1024
    );

    let mut arm = pose::Arm::new(cfg.size, cfg.shadow);
    let mut buf = Vec::new();
    let (mut t_tiles, mut t_shadow) = (Duration::ZERO, Duration::ZERO);
    let mut times = Vec::new();
    let mut bytes = Vec::new();
    let mut tile_counts = Vec::new();
    let mut shots = Vec::new();
    let n = 1200;
    for i in 0..n {
        let t = i as f32 / n as f32 * std::f32::consts::TAU * 2.0;
        let cursor = (w as f32 * (0.5 + 0.45 * t.cos()), h as f32 * (0.5 + 0.42 * (1.7 * t).sin()));
        if i % 100 == 0 {
            arm.press();
        } else if i % 100 == 8 {
            arm.release();
        }
        let p = arm.step(cursor, w as f32, h as f32, 1.0 / 120.0, &sprite.meta);
        let plan = raster::plan(&p, &sprite, w, h, scale);
        let keep = i % 150 == 30;
        let mut shot = Vec::new();
        let start = Instant::now();
        let mut total = 0;
        if let Some(f) = plan.shadow {
            buf.resize(f.bw * f.bh, 0u32);
            raster::render_shadow(&p, &sprite, &f, h as f32, scale, &mut buf);
            total += f.bw * f.bh * 4;
            if keep {
                shot.push((f, buf.clone()));
            }
        }
        let mid = Instant::now();
        for tile in &plan.tiles {
            let f = tile.frame;
            buf.resize(f.bw * f.bh, 0u32);
            raster::render_tile(&p, &sprite, tile, h as f32, scale, &mut buf);
            total += f.bw * f.bh * 4;
            if keep {
                shot.push((f, buf.clone()));
            }
        }
        let end = Instant::now();
        t_shadow += mid - start;
        t_tiles += end - mid;
        times.push(end - start);
        bytes.push(total);
        tile_counts.push(plan.tiles.len());
        if keep {
            shots.push((shot, cursor));
        }
    }
    let n_t = times.len() as u32;
    times.sort();
    let pct = |q: f64| times[((times.len() - 1) as f64 * q) as usize];
    println!(
        "frame x{}: avg {:.0} us (shadow {:.0}, tiles {:.0}), p50 {:.0} us, p99 {:.0} us, max {:.0} us",
        n_t,
        us(times.iter().sum::<Duration>() / n_t),
        us(t_shadow / n_t),
        us(t_tiles / n_t),
        us(pct(0.5)),
        us(pct(0.99)),
        us(*times.last().unwrap()),
    );
    println!(
        "buffers per frame: avg {} KB, max {} KB; tiles avg {:.1}, max {}",
        bytes.iter().sum::<usize>() / bytes.len() / 1024,
        bytes.iter().max().unwrap() / 1024,
        tile_counts.iter().sum::<usize>() as f32 / tile_counts.len() as f32,
        tile_counts.iter().max().unwrap()
    );
    if let Some(hwm) = fs::read_to_string("/proc/self/status").ok().and_then(|s| {
        s.lines().find(|l| l.starts_with("VmHWM")).map(|l| l.split_whitespace().nth(1).unwrap_or("?").to_string())
    }) {
        println!("peak memory (VmHWM): {hwm} KB");
    }

    if let Some(out) = out {
        let (cw, ch) = ((w as f32 * scale) as usize, (h as f32 * scale) as usize);
        let mut canvas: Vec<u32> = (0..cw * ch)
            .map(|i| {
                let g = 215 + (i / cw) as u32 * 40 / ch as u32;
                0xff000000 | g << 16 | g << 8 | g
            })
            .collect();
        for (layers, cursor) in &shots {
            for (f, px) in layers {
                composite(&mut canvas, cw, ch, f, px, scale);
            }
            let (px_, py_) = ((cursor.0 * scale) as i64, ((h as f32 - cursor.1) * scale) as i64);
            for d in -12i64..=12 {
                for (x, y) in [(px_ + d, py_), (px_, py_ + d)] {
                    if x >= 0 && y >= 0 && (x as usize) < cw && (y as usize) < ch {
                        canvas[y as usize * cw + x as usize] = 0xffff0000;
                    }
                }
            }
        }
        let rgba: Vec<u8> = canvas.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8, 255]).collect();
        let file = fs::File::create(out).map_err(|e| e.to_string())?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), cw as u32, ch as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().and_then(|mut w| w.write_image_data(&rgba)).map_err(|e| e.to_string())?;
        println!("wrote {}", out.display());
    }
    Ok(())
}

/// Blend a buffer onto an opaque canvas, stretching it to its frame with bilinear
/// filtering like a compositor would.
fn composite(canvas: &mut [u32], cw: usize, ch: usize, f: &raster::Frame, px: &[u32], scale: f32) {
    let (x0, y0) = ((f.x as f32 * scale) as usize, (f.y as f32 * scale) as usize);
    let (dw, dh) = ((f.w as f32 * scale) as usize, (f.h as f32 * scale) as usize);
    let (kx, ky) = (f.bw as f32 / dw as f32, f.bh as f32 / dh as f32);
    let get = |x: i64, y: i64| -> [f32; 4] {
        let x = x.clamp(0, f.bw as i64 - 1) as usize;
        let y = y.clamp(0, f.bh as i64 - 1) as usize;
        let p = px[y * f.bw + x];
        [(p >> 24) as f32, ((p >> 16) & 255) as f32, ((p >> 8) & 255) as f32, (p & 255) as f32]
    };
    for dy in 0..dh {
        for dx in 0..dw {
            let (cx, cy) = (x0 + dx, y0 + dy);
            if cx >= cw || cy >= ch {
                continue;
            }
            let u = (dx as f32 + 0.5) * kx - 0.5;
            let v = (dy as f32 + 0.5) * ky - 0.5;
            let (ui, vi) = (u.floor() as i64, v.floor() as i64);
            let (fu, fv) = (u - u.floor(), v - v.floor());
            let (a, b, c, d) = (get(ui, vi), get(ui + 1, vi), get(ui, vi + 1), get(ui + 1, vi + 1));
            let s: Vec<f32> =
                (0..4).map(|k| (a[k] + (b[k] - a[k]) * fu) * (1.0 - fv) + (c[k] + (d[k] - c[k]) * fu) * fv).collect();
            let ia = 1.0 - s[0] / 255.0;
            let dst = &mut canvas[cy * cw + cx];
            let ch_ = |sh: u32, k: usize| ((s[k] + ((*dst >> sh) & 255) as f32 * ia).round() as u32).min(255);
            *dst = 0xff000000 | ch_(16, 1) << 16 | ch_(8, 2) << 8 | ch_(0, 3);
        }
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}
fn us(d: Duration) -> f64 {
    d.as_secs_f64() * 1e6
}
