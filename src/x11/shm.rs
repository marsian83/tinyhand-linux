//! Pixel staging: where frames are rendered before the X server copies them into our
//! windows. MIT-SHM when the server shares memory with us (local displays), plain
//! PutImage otherwise (e.g. `ssh -X`).
//!
//! A frame reserves its total size up front, then carves out one slot per window.

use std::{
    ffi::c_void,
    os::fd::OwnedFd,
    ptr::NonNull,
    time::{Duration, Instant},
};

use rustix::{
    fs::{ftruncate, memfd_create, MemfdFlags},
    mm::{mmap, munmap, MapFlags, ProtFlags},
};
use x11rb::{
    connection::Connection as _,
    protocol::{
        shm::ConnectionExt as _,
        xproto::{ConnectionExt as _, Drawable, Gcontext, ImageFormat, Pixmap},
    },
    rust_connection::RustConnection,
};

use super::{X11Ext, XResult};

/// If the server hasn't confirmed an SHM frame by then, assume the confirmation got lost
/// (an error ate it) rather than stop drawing for good.
const IN_FLIGHT_LIMIT: Duration = Duration::from_millis(250);

/// A shared-memory segment the server reads pixels from.
pub(super) struct ShmSeg {
    id: u32,
    ptr: NonNull<u32>,
    /// Capacity in pixels.
    cap: usize,
}

impl ShmSeg {
    pub fn create(conn: &RustConnection, px: usize) -> XResult<ShmSeg> {
        let bytes = px * 4;
        let fd: OwnedFd = memfd_create(c"tinyhand-x11", MemfdFlags::CLOEXEC).x11()?;
        ftruncate(&fd, bytes as u64).x11()?;
        // SAFETY: a fresh shared mapping of our own memfd; nothing else aliases it.
        let ptr =
            unsafe { mmap(std::ptr::null_mut(), bytes, ProtFlags::READ | ProtFlags::WRITE, MapFlags::SHARED, &fd, 0) }
                .x11()?;
        let ptr = NonNull::new(ptr.cast::<u32>()).ok_or("X11: mmap returned null")?;
        // Owned from here on: an error below unmaps it in Drop.
        let seg = ShmSeg { id: conn.generate_id().x11()?, ptr, cap: px };
        conn.shm_attach_fd(seg.id, fd, false).x11()?;
        Ok(seg)
    }

    pub fn id(&self) -> u32 {
        self.id
    }

    /// Detach from the server, then unmap (in Drop).
    pub fn free(self, conn: &RustConnection) {
        let _ = conn.shm_detach(self.id);
    }
}

impl Drop for ShmSeg {
    fn drop(&mut self) {
        // SAFETY: ptr/cap describe our own mapping, and no slice of it outlives `self`.
        let _ = unsafe { munmap(self.ptr.as_ptr().cast::<c_void>(), self.cap * 4) };
    }
}

enum Store {
    Shm(Option<ShmSeg>),
    Heap(Vec<u32>),
}

pub(super) struct Staging {
    store: Store,
    /// Pixels reserved for the current frame, and handed out so far.
    reserved: usize,
    used: usize,
    /// When an SHM frame was handed to the server (it may still be reading it).
    in_flight: Option<Instant>,
    /// 1×1 pixmap that the completion marker is drawn into.
    marker: Pixmap,
    /// The server wants the other byte order.
    swap: bool,
    max_request: usize,
}

impl Staging {
    pub fn new(seg: Option<ShmSeg>, marker: Pixmap, swap: bool, max_request: usize) -> Staging {
        let store = match seg {
            Some(s) => Store::Shm(Some(s)),
            None => Store::Heap(Vec::new()),
        };
        Staging { store, reserved: 0, used: 0, in_flight: None, marker, swap, max_request }
    }

    pub fn is_shm(&self) -> bool {
        matches!(self.store, Store::Shm(_))
    }

    /// The server may still be reading the last frame: don't write until it's done.
    pub fn busy(&mut self, now: Instant) -> bool {
        if self.in_flight.is_some_and(|t| now - t > IN_FLIGHT_LIMIT) {
            self.in_flight = None;
        }
        self.in_flight.is_some()
    }

    /// When to give up waiting for the server's confirmation.
    pub fn busy_until(&self) -> Option<Instant> {
        self.in_flight.map(|t| t + IN_FLIGHT_LIMIT)
    }

    /// The server confirmed it read the last frame (ShmCompletion).
    pub fn done(&mut self) {
        self.in_flight = None;
    }

    /// Start a frame needing `px` pixels in total. Only call when not busy.
    pub fn reserve(&mut self, conn: &RustConnection, px: usize) -> XResult<()> {
        self.used = 0;
        self.reserved = px;
        match &mut self.store {
            Store::Shm(seg) => {
                if seg.as_ref().is_none_or(|s| s.cap < px) {
                    if let Some(old) = seg.take() {
                        old.free(conn);
                    }
                    // Headroom so a slightly bigger frame doesn't remap every time.
                    let cap = (px + px / 2).max(64 * 1024).next_multiple_of(16 * 1024);
                    *seg = Some(ShmSeg::create(conn, cap)?);
                }
            }
            Store::Heap(v) => {
                if v.len() < px {
                    v.resize(px, 0);
                }
            }
        }
        Ok(())
    }

    /// Hand out the next `n` pixels of the frame; returns the slot's start.
    ///
    /// # Panics
    /// If the frame's slots add up to more than `reserve` asked for (a bug in the
    /// caller's count): writing past the shared segment must never happen.
    pub fn alloc(&mut self, n: usize) -> usize {
        assert!(self.used + n <= self.reserved, "staging: {} + {n} px > {} reserved", self.used, self.reserved);
        let at = self.used;
        self.used += n;
        at
    }

    /// A slot handed out by `alloc`.
    pub fn slice(&mut self, at: usize, n: usize) -> &mut [u32] {
        assert!(at + n <= self.used, "staging: slot {at}+{n} wasn't allocated");
        match &mut self.store {
            Store::Shm(Some(s)) => {
                assert!(at + n <= s.cap);
                // SAFETY: in bounds (checked above); slots don't overlap, and the server
                // isn't reading the segment (callers check `busy` before a frame).
                unsafe { std::slice::from_raw_parts_mut(s.ptr.as_ptr().add(at), n) }
            }
            Store::Shm(None) => &mut [],
            Store::Heap(v) => &mut v[at..at + n],
        }
    }

    /// Upload a w×h slot to (dx, dy) in `d`. Returns the last request's sequence number.
    #[allow(clippy::too_many_arguments)]
    pub fn put(
        &mut self,
        conn: &RustConnection,
        gc: Gcontext,
        d: Drawable,
        w: usize,
        h: usize,
        dx: i16,
        dy: i16,
        at: usize,
    ) -> XResult<u64> {
        if w == 0 || h == 0 {
            return Ok(0);
        }
        if self.swap {
            for p in self.slice(at, w * h) {
                *p = p.swap_bytes();
            }
        }
        let mut seq = 0;
        match &self.store {
            Store::Shm(Some(s)) => {
                let (w, h) = (w as u16, h as u16);
                seq = conn
                    .shm_put_image(
                        d,
                        gc,
                        w,
                        h,
                        0,
                        0,
                        w,
                        h,
                        dx,
                        dy,
                        32,
                        ImageFormat::Z_PIXMAP.into(),
                        false,
                        s.id,
                        (at * 4) as u32,
                    )
                    .x11()?
                    .sequence_number();
            }
            Store::Shm(None) => {}
            Store::Heap(v) => {
                // Plain PutImage, split into requests the server will take.
                let rows = ((self.max_request.saturating_sub(64)) / (w * 4)).max(1);
                let px = &v[at..at + w * h];
                for y0 in (0..h).step_by(rows) {
                    let y1 = (y0 + rows).min(h);
                    let bytes = as_bytes(&px[y0 * w..y1 * w]);
                    seq = conn
                        .put_image(
                            ImageFormat::Z_PIXMAP,
                            d,
                            gc,
                            w as u16,
                            (y1 - y0) as u16,
                            dx,
                            dy + y0 as i16,
                            0,
                            32,
                            bytes,
                        )
                        .x11()?
                        .sequence_number();
                }
            }
        }
        Ok(seq)
    }

    /// End the frame. With SHM, ask for a completion event once the server is done.
    pub fn finish(&mut self, conn: &RustConnection, gc: Gcontext, now: Instant) -> XResult<()> {
        if let Store::Shm(Some(s)) = &self.store {
            if self.used > 0 {
                // A 1-pixel put after everything else: its completion means all are read.
                conn.shm_put_image(
                    self.marker,
                    gc,
                    1,
                    1,
                    0,
                    0,
                    1,
                    1,
                    0,
                    0,
                    32,
                    ImageFormat::Z_PIXMAP.into(),
                    true,
                    s.id,
                    0,
                )
                .x11()?;
                self.in_flight = Some(now);
            }
        }
        Ok(())
    }

    /// Staging memory in KB (for --stats).
    pub fn kb(&self) -> usize {
        match &self.store {
            Store::Shm(Some(s)) => s.cap * 4 / 1024,
            Store::Heap(v) => v.len() * 4 / 1024,
            Store::Shm(None) => 0,
        }
    }

    pub fn free(&mut self, conn: &RustConnection) {
        if let Store::Shm(seg) = &mut self.store {
            if let Some(s) = seg.take() {
                s.free(conn);
            }
        }
    }
}

fn as_bytes(px: &[u32]) -> &[u8] {
    // SAFETY: u32 → u8 view of the same memory, length scaled; u8 has no alignment needs.
    unsafe { std::slice::from_raw_parts(px.as_ptr().cast::<u8>(), px.len() * 4) }
}
