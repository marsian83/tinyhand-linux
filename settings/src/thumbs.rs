//! Hand thumbnails. Hand images are big (1024×1536), so they are decoded and scaled on
//! worker threads, a few at a time, and each image widget gets its thumbnail as soon as
//! it's ready. Finished thumbnails are cached as textures for the life of the window.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;

use gtk::gdk_pixbuf::{Colorspace, InterpType, Pixbuf};
use gtk::{gdk, gio, glib, prelude::*};
use tinyhand::hand;

/// Thumbnail side in device pixels: sharp at 2× for the 56 px tiles.
const SIDE: i32 = 112;
/// Images decoded at once. Each one needs ~6 MB while it's being decoded.
const WORKERS: usize = 2;

/// Straight (not premultiplied) RGBA pixels, safe to send between threads.
pub struct Rgba {
    width: i32,
    height: i32,
    stride: usize,
    bytes: glib::Bytes,
}

impl Rgba {
    pub fn texture(&self) -> gdk::Texture {
        gdk::MemoryTexture::new(self.width, self.height, gdk::MemoryFormat::R8g8b8a8, &self.bytes, self.stride).upcast()
    }
}

/// Scale the area `(x, y, w, h)` of an image (it may reach past the edges, which stay
/// transparent) to `out_w`×`out_h` pixels.
pub fn scaled(rgba: Vec<u8>, width: usize, height: usize, area: (f64, f64, f64, f64), out: (i32, i32)) -> Option<Rgba> {
    let (w, h) = (i32::try_from(width).ok()?, i32::try_from(height).ok()?);
    if rgba.len() != width * height * 4 || w == 0 || h == 0 || area.2 <= 0.0 || area.3 <= 0.0 {
        return None;
    }
    let src = Pixbuf::from_bytes(&glib::Bytes::from_owned(rgba), Colorspace::Rgb, true, 8, w, h, w * 4);
    let dst = Pixbuf::new(Colorspace::Rgb, true, 8, out.0, out.1)?;
    dst.fill(0);
    let (sx, sy) = (out.0 as f64 / area.2, out.1 as f64 / area.3);
    let (ox, oy) = (-area.0 * sx, -area.1 * sy);
    // Only the part of the output the image covers.
    let x0 = ox.max(0.0).floor() as i32;
    let y0 = oy.max(0.0).floor() as i32;
    let x1 = (ox + w as f64 * sx).min(out.0 as f64).ceil() as i32;
    let y1 = (oy + h as f64 * sy).min(out.1 as f64).ceil() as i32;
    if x1 > x0 && y1 > y0 {
        src.scale(&dst, x0, y0, x1 - x0, y1 - y0, ox, oy, sx, sy, InterpType::Bilinear);
    }
    let stride = usize::try_from(dst.rowstride()).ok()?;
    Some(Rgba { width: out.0, height: out.1, stride, bytes: dst.read_pixel_bytes() })
}

/// A square around the hand (not the stick), like the toolbar's and the macOS app's.
fn render(path: PathBuf) -> Option<Rgba> {
    let img = hand::load_png(&path).ok()?;
    let (w, h) = (img.w as f64, img.h as f64);
    let area = match hand::measure(&img) {
        Some(m) => {
            let hand_w = (m.hand_max_x - m.hand_min_x) as f64 * w;
            let hand_h = (m.wrist - m.hand_top) as f64 * h;
            let side = hand_w.max(hand_h) * 1.06;
            let cx = (m.hand_min_x + m.hand_max_x) as f64 / 2.0 * w;
            let cy = (m.hand_top + m.wrist) as f64 / 2.0 * h;
            (cx - side / 2.0, cy - side / 2.0, side, side)
        }
        // Not shaped like a hand on a stick: show the whole image.
        None => {
            let side = w.max(h);
            ((w - side) / 2.0, (h - side) / 2.0, side, side)
        }
    };
    scaled(img.rgba, img.w, img.h, area, (SIDE, SIDE))
}

#[derive(Default)]
pub struct Thumbs {
    cache: RefCell<HashMap<String, gdk::Texture>>,
    /// Hands waiting to load, in order, and the image widgets waiting for each.
    queue: RefCell<VecDeque<String>>,
    waiting: RefCell<HashMap<String, Vec<glib::WeakRef<gtk::Image>>>>,
    workers: Cell<usize>,
}

impl Thumbs {
    pub fn new() -> Rc<Thumbs> {
        Rc::default()
    }

    /// Show the thumbnail of hand `id` in `image`: now if it's loaded, else once it is.
    pub fn show(self: &Rc<Self>, id: &str, image: &gtk::Image) {
        let mut waiting = self.waiting.borrow_mut();
        // It may still be waiting for the hand it showed before.
        for images in waiting.values_mut() {
            images.retain(|w| w.upgrade().is_some_and(|i| i != *image));
        }
        if let Some(t) = self.cache.borrow().get(id) {
            image.set_paintable(Some(t));
            return;
        }
        waiting.entry(id.to_string()).or_default().push(image.downgrade());
        let mut queue = self.queue.borrow_mut();
        if !queue.iter().any(|q| q == id) {
            queue.push_back(id.to_string());
        }
        drop(queue);
        drop(waiting);
        if self.workers.get() < WORKERS {
            self.workers.set(self.workers.get() + 1);
            glib::spawn_future_local(Rc::clone(self).work());
        }
    }

    /// Drop a cached thumbnail (a custom hand was removed).
    pub fn forget(&self, id: &str) {
        self.cache.borrow_mut().remove(id);
    }

    async fn work(self: Rc<Self>) {
        loop {
            let Some(id) = self.queue.borrow_mut().pop_front() else { break };
            let texture = match hand::find(&id) {
                Some(path) => gio::spawn_blocking(move || render(path)).await.ok().flatten().map(|t| t.texture()),
                None => None,
            };
            let images = self.waiting.borrow_mut().remove(&id).unwrap_or_default();
            let Some(texture) = texture else { continue };
            for p in images.iter().filter_map(|p| p.upgrade()) {
                p.set_paintable(Some(&texture));
            }
            self.cache.borrow_mut().insert(id, texture);
        }
        self.workers.set(self.workers.get() - 1);
    }
}
