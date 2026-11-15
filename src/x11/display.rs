//! The X connection, what the server supports, and the windows we draw into.

use std::collections::HashSet;

use x11rb::{
    connection::{Connection as _, RequestConnection as _},
    protocol::{
        randr::{self, ConnectionExt as _},
        render::{self, ConnectionExt as _},
        shape::{self, ConnectionExt as _},
        shm::ConnectionExt as _,
        xfixes::ConnectionExt as _,
        xinput::{self, ConnectionExt as _},
        xproto::*,
        xtest::ConnectionExt as _,
    },
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
    NONE,
};

use super::{
    shm::{ShmSeg, Staging},
    Geom, X11Ext, XResult, PARKED,
};

/// One ARGB window: an arm tile, the shadow, an ink tile or the toolbar.
#[derive(Clone, Copy, Debug)]
pub(super) struct Win {
    pub id: Window,
    pub geom: Geom,
    /// XRender picture on the window (shadow only), else NONE.
    pub pict: render::Picture,
    /// Sequence number of the request that last drew the window's pixels: exposures
    /// generated before it are already repainted.
    pub seq: u64,
}

/// A monitor, in root-window device px.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Monitor {
    pub x: i16,
    pub y: i16,
    pub w: u16,
    pub h: u16,
}

impl Monitor {
    /// Identity for the session: monitors with the same geometry are the same monitor.
    pub fn id(&self) -> u64 {
        (self.x as u16 as u64) << 48 | (self.y as u16 as u64) << 32 | (self.w as u64) << 16 | self.h as u64
    }
}

/// Optional extensions the server has.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Extensions {
    pub xtest: bool,
    pub xfixes: bool,
    pub xi2: bool,
    pub randr: bool,
}

struct Atoms {
    wm_type: Atom,
    wm_type_dnd: Atom,
    compton_shadow: Atom,
    wm_name: Atom,
    utf8: Atom,
    opacity: Atom,
}

pub(super) struct Display {
    pub conn: RustConnection,
    pub root: Window,
    pub screen_size: (u16, u16),
    visual: Visualid,
    cmap: Colormap,
    pub gc: Gcontext,
    /// 32-bit ARGB XRender format (bilinear shadow scaling), if XRender is there.
    pub pictfmt: Option<render::Pictformat>,
    atoms: Atoms,
    pub ext: Extensions,
    pub xwayland: bool,
    /// A compositing manager runs: windows can be translucent.
    pub composited: bool,
    pub staging: Staging,
    /// Our windows (to tell them from others in stacking events).
    pub ours: HashSet<Window>,
    /// Our windows' stacking order needs fixing.
    pub restack: bool,
}

impl Display {
    pub fn open() -> XResult<Display> {
        let (conn, screen_num) = x11rb::connect(None).map_err(|e| format!("can't open X display: {e}"))?;
        let setup = conn.setup();
        let screen = setup.roots[screen_num].clone();
        let root = screen.root;
        let swap = (setup.image_byte_order == ImageOrder::MSB_FIRST) == cfg!(target_endian = "little");
        let max_request = conn.maximum_request_bytes();

        let has = |name: &'static str| conn.extension_information(name).ok().flatten().is_some();
        let xwayland = has("XWAYLAND");

        // 32-bit TrueColor visual for ARGB windows.
        let visual = screen
            .allowed_depths
            .iter()
            .filter(|d| d.depth == 32)
            .flat_map(|d| d.visuals.iter())
            .find(|v| {
                v.class == VisualClass::TRUE_COLOR
                    && v.red_mask == 0xff0000
                    && v.green_mask == 0xff00
                    && v.blue_mask == 0xff
            })
            .map(|v| v.visual_id)
            .ok_or("X11: no 32-bit TrueColor visual")?;

        if !has(shape::X11_EXTENSION_NAME) {
            return Err("X11: the SHAPE extension is missing".into());
        }
        let sv = conn.shape_query_version().x11()?.reply().x11()?;
        if (sv.major_version, sv.minor_version) < (1, 1) {
            return Err("X11: SHAPE 1.1 (input shapes) is required".into());
        }

        let ext = Extensions {
            xfixes: has("XFIXES") && conn.xfixes_query_version(5, 0).x11()?.reply().is_ok_and(|r| r.major_version >= 4),
            xtest: has("XTEST") && conn.xtest_get_version(2, 2).x11()?.reply().is_ok(),
            randr: has(randr::X11_EXTENSION_NAME)
                && conn
                    .randr_query_version(1, 5)
                    .x11()?
                    .reply()
                    .is_ok_and(|r| (r.major_version, r.minor_version) >= (1, 5)),
            xi2: has(xinput::X11_EXTENSION_NAME)
                && conn
                    .xinput_xi_query_version(2, 2)
                    .x11()?
                    .reply()
                    .is_ok_and(|r| (r.major_version, r.minor_version) >= (2, 1)),
        };
        let pictfmt = if has(render::X11_EXTENSION_NAME) && conn.render_query_version(0, 11).x11()?.reply().is_ok() {
            argb_format(&conn)
        } else {
            None
        };

        // A compositing manager owns _NET_WM_CM_S<screen>. XWayland always composites.
        let cm = conn.intern_atom(false, format!("_NET_WM_CM_S{screen_num}").as_bytes()).x11()?.reply().x11()?.atom;
        let composited = xwayland || conn.get_selection_owner(cm).x11()?.reply().x11()?.owner != NONE;

        let atom = |n: &str| -> XResult<Atom> { Ok(conn.intern_atom(false, n.as_bytes()).x11()?.reply().x11()?.atom) };
        let atoms = Atoms {
            wm_type: atom("_NET_WM_WINDOW_TYPE")?,
            wm_type_dnd: atom("_NET_WM_WINDOW_TYPE_DND")?,
            compton_shadow: atom("_COMPTON_SHADOW")?,
            wm_name: atom("_NET_WM_NAME")?,
            utf8: atom("UTF8_STRING")?,
            opacity: atom("_NET_WM_WINDOW_OPACITY")?,
        };

        let cmap = conn.generate_id().x11()?;
        conn.create_colormap(ColormapAlloc::NONE, cmap, root, visual).x11()?;
        // A depth-32 GC needs a depth-32 drawable; the 1×1 pixmap doubles as the SHM
        // completion marker's target.
        let marker = conn.generate_id().x11()?;
        conn.create_pixmap(32, marker, root, 1, 1).x11()?;
        let gc = conn.generate_id().x11()?;
        conn.create_gc(gc, marker, &CreateGCAux::new().graphics_exposures(0)).x11()?;

        // MIT-SHM with fd passing (1.2+); falls back to plain PutImage (e.g. ssh -X).
        let mut seg = None;
        if has("MIT-SHM")
            && conn.shm_query_version().x11()?.reply().is_ok_and(|r| (r.major_version, r.minor_version) >= (1, 2))
        {
            if let Ok(s) = ShmSeg::create(&conn, 64 * 1024) {
                if conn.get_input_focus().x11()?.reply().is_ok() && segment_works(&conn, root, s.id()) {
                    seg = Some(s);
                } else {
                    s.free(&conn);
                }
            }
        }

        Ok(Display {
            screen_size: (screen.width_in_pixels, screen.height_in_pixels),
            staging: Staging::new(seg, marker, swap, max_request),
            conn,
            root,
            visual,
            cmap,
            gc,
            pictfmt,
            atoms,
            ext,
            xwayland,
            composited,
            ours: HashSet::new(),
            restack: true,
        })
    }

    /// The full-screen window that catches input while the pointer is on. On a real X
    /// server it's InputOnly (no pixels); XWayland only routes pointer input to windows
    /// with a surface, so there it's a transparent InputOutput window.
    pub fn create_sink(&mut self) -> XResult<Window> {
        let c = &self.conn;
        let id = c.generate_id().x11()?;
        let events = EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION;
        let cursor = invisible_cursor(c, self.root)?;
        let (w, h) = self.screen_size;
        let input_output = match std::env::var("TINYHAND_X11_INPUT").as_deref() {
            Ok("only") => false,
            Ok("output") => true,
            _ => self.xwayland,
        };
        if input_output {
            let aux = CreateWindowAux::new()
                .background_pixel(0)
                .border_pixel(0)
                .colormap(self.cmap)
                .override_redirect(1)
                .event_mask(events)
                .cursor(cursor);
            c.create_window(32, id, self.root, 0, 0, w, h, 0, WindowClass::INPUT_OUTPUT, self.visual, &aux).x11()?;
        } else {
            let aux = CreateWindowAux::new().override_redirect(1).event_mask(events).cursor(cursor);
            c.create_window(0, id, self.root, 0, 0, w, h, 0, WindowClass::INPUT_ONLY, 0u32, &aux).x11()?;
        }
        c.free_cursor(cursor).x11()?;
        self.ours.insert(id);
        Ok(id)
    }

    /// Watch the root for windows stacking above ours, monitor changes, and (XI2) Alt.
    pub fn watch_root(&self) -> XResult<()> {
        let c = &self.conn;
        c.change_window_attributes(
            self.root,
            &ChangeWindowAttributesAux::new().event_mask(EventMask::SUBSTRUCTURE_NOTIFY),
        )
        .x11()?;
        if self.ext.randr {
            c.randr_select_input(self.root, randr::NotifyMask::SCREEN_CHANGE).x11()?;
        }
        if self.ext.xi2 {
            let mask = xinput::XIEventMask::RAW_KEY_PRESS | xinput::XIEventMask::RAW_KEY_RELEASE;
            c.xinput_xi_select_events(
                self.root,
                &[xinput::EventMask { deviceid: xinput::Device::ALL_MASTER.into(), mask: vec![mask] }],
            )
            .x11()?;
        }
        Ok(())
    }

    /// Keycodes of the Alt (Mod1) keys.
    pub fn mod1_keys(&self) -> XResult<Vec<u32>> {
        let mods = self.conn.get_modifier_mapping().x11()?.reply().x11()?;
        let k = mods.keycodes_per_modifier() as usize;
        Ok(mods.keycodes[3 * k..4 * k].iter().filter(|&&c| c != 0).map(|&c| c as u32).collect())
    }

    /// Root pointer position and whether Alt is held.
    pub fn pointer(&self) -> XResult<((i16, i16), bool)> {
        let p = self.conn.query_pointer(self.root).x11()?.reply().x11()?;
        Ok(((p.root_x, p.root_y), u16::from(p.mask) & u16::from(KeyButMask::MOD1) != 0))
    }

    pub fn root_size(&self) -> XResult<(u16, u16)> {
        let g = self.conn.get_geometry(self.root).x11()?.reply().x11()?;
        Ok((g.width, g.height))
    }

    /// Monitors (RandR 1.5, else the whole root) and the fastest refresh rate (Hz, if known).
    pub fn monitors(&self) -> XResult<(Vec<Monitor>, Option<f64>)> {
        let c = &self.conn;
        let mut mons = Vec::new();
        let mut hz: f64 = 0.0;
        if self.ext.randr {
            if let Ok(r) = c.randr_get_monitors(self.root, true).x11()?.reply() {
                mons = r
                    .monitors
                    .iter()
                    .map(|m| Monitor { x: m.x, y: m.y, w: m.width, h: m.height })
                    .filter(|m| m.w > 0 && m.h > 0)
                    .collect();
            }
            if let Ok(res) = c.randr_get_screen_resources_current(self.root).x11()?.reply() {
                for &crtc in &res.crtcs {
                    let Ok(info) = c.randr_get_crtc_info(crtc, res.config_timestamp).x11()?.reply() else { continue };
                    if let Some(m) = res.modes.iter().find(|m| m.id == info.mode) {
                        let mut v = m.vtotal as f64;
                        if u32::from(m.mode_flags) & u32::from(randr::ModeFlag::DOUBLE_SCAN) != 0 {
                            v *= 2.0;
                        }
                        if u32::from(m.mode_flags) & u32::from(randr::ModeFlag::INTERLACE) != 0 {
                            v /= 2.0;
                        }
                        if m.htotal > 0 && v > 0.0 {
                            hz = hz.max(m.dot_clock as f64 / (m.htotal as f64 * v));
                        }
                    }
                }
            }
        }
        if mons.is_empty() {
            let (w, h) = self.root_size()?;
            mons.push(Monitor { x: 0, y: 0, w, h });
        }
        Ok((mons, (20.0..=1000.0).contains(&hz).then_some(hz)))
    }

    /// UI scale from Xft.dpi in RESOURCE_MANAGER (96 dpi = 1), in quarter steps.
    pub fn xft_scale(&self) -> f32 {
        let reply = self
            .conn
            .get_property(false, self.root, AtomEnum::RESOURCE_MANAGER, AtomEnum::STRING, 0, 1 << 20)
            .map(|c| c.reply());
        let Ok(Ok(r)) = reply else { return 1.0 };
        let text = String::from_utf8_lossy(&r.value);
        let dpi = text.lines().find_map(|l| l.strip_prefix("Xft.dpi:").and_then(|v| v.trim().parse::<f32>().ok()));
        dpi.map_or(1.0, |d| ((d / 96.0) * 4.0).round().clamp(4.0, 16.0) / 4.0)
    }

    // ------------------------------------------------------------------ windows

    /// A new ARGB window, parked off screen, that never takes input.
    pub fn new_win(&mut self, with_pict: bool) -> XResult<Win> {
        let c = &self.conn;
        let id = c.generate_id().x11()?;
        // Exposures (the server clearing a window to its background, e.g. when XWayland
        // sets up its surface or, without a compositor, when windows overlap) trigger a
        // redraw of what was there.
        let aux = CreateWindowAux::new()
            .background_pixel(0)
            .border_pixel(0)
            .colormap(self.cmap)
            .override_redirect(1)
            .event_mask(EventMask::EXPOSURE);
        let (x, y, w, h) = PARKED;
        c.create_window(32, id, self.root, x, y, w, h, 0, WindowClass::INPUT_OUTPUT, self.visual, &aux).x11()?;
        // Never take input: clicks go through to the sink underneath.
        c.shape_rectangles(shape::SO::SET, shape::SK::INPUT, ClipOrdering::UNSORTED, id, 0, 0, &[]).x11()?;
        // Ask compositors for no shadows/effects (DND icons get none; picom honours
        // _COMPTON_SHADOW).
        c.change_property32(PropMode::REPLACE, id, self.atoms.wm_type, AtomEnum::ATOM, &[self.atoms.wm_type_dnd])
            .x11()?;
        c.change_property32(PropMode::REPLACE, id, self.atoms.compton_shadow, AtomEnum::CARDINAL, &[0]).x11()?;
        c.change_property8(PropMode::REPLACE, id, self.atoms.wm_name, self.atoms.utf8, b"tinyhand").x11()?;
        c.change_property8(PropMode::REPLACE, id, AtomEnum::WM_CLASS, AtomEnum::STRING, b"tinyhand\0tinyhand\0")
            .x11()?;
        let mut pict = NONE;
        if with_pict {
            if let Some(fmt) = self.pictfmt {
                pict = c.generate_id().x11()?;
                c.render_create_picture(pict, id, fmt, &render::CreatePictureAux::new()).x11()?;
            }
        }
        // Mapped once and parked when unused: compositors don't play map/unmap
        // animations on it.
        c.map_window(id).x11()?;
        self.ours.insert(id);
        self.restack = true;
        Ok(Win { id, geom: PARKED, pict, seq: 0 })
    }

    pub fn destroy_win(&mut self, w: Win) {
        if w.pict != NONE {
            let _ = self.conn.render_free_picture(w.pict);
        }
        let _ = self.conn.destroy_window(w.id);
        self.ours.remove(&w.id);
    }

    /// Move/resize a window (only if it changed).
    pub fn place(&self, w: &mut Win, g: Geom) -> XResult<()> {
        if w.geom != g {
            let aux = ConfigureWindowAux::new().x(g.0 as i32).y(g.1 as i32).width(g.2 as u32).height(g.3 as u32);
            self.conn.configure_window(w.id, &aux).x11()?;
            w.geom = g;
        }
        Ok(())
    }

    /// Move/resize a window and upload its pixels from staging slot `at` (bw×bh).
    pub fn show(&mut self, win: &mut Win, g: Geom, at: usize, bw: usize, bh: usize) -> XResult<()> {
        if !self.composited {
            // No alpha without a compositor: clip the window to its opaque pixels.
            let rects = alpha_rects(self.staging.slice(at, bw * bh), bw, bh);
            self.conn
                .shape_rectangles(shape::SO::SET, shape::SK::BOUNDING, ClipOrdering::YX_BANDED, win.id, 0, 0, &rects)
                .x11()?;
        }
        self.place(win, g)?;
        win.seq = self.staging.put(&self.conn, self.gc, win.id, bw, bh, 0, 0, at)?;
        Ok(())
    }

    /// Window opacity through the compositor (_NET_WM_WINDOW_OPACITY), for fading
    /// without redrawing.
    pub fn set_opacity(&self, win: Window, a: f32) -> XResult<()> {
        let v = (a.clamp(0.0, 1.0) as f64 * u32::MAX as f64) as u32;
        self.conn.change_property32(PropMode::REPLACE, win, self.atoms.opacity, AtomEnum::CARDINAL, &[v]).x11()?;
        Ok(())
    }

    /// Stack `order` bottom to top, above everything else.
    pub fn stack(&self, order: &[Window]) -> XResult<()> {
        let Some(&first) = order.first() else { return Ok(()) };
        self.conn.configure_window(first, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE)).x11()?;
        for pair in order.windows(2) {
            self.conn
                .configure_window(pair[1], &ConfigureWindowAux::new().sibling(pair[0]).stack_mode(StackMode::ABOVE))
                .x11()?;
        }
        Ok(())
    }
}

fn argb_format(conn: &RustConnection) -> Option<render::Pictformat> {
    let r = conn.render_query_pict_formats().ok()?.reply().ok()?;
    r.formats
        .iter()
        .find(|f| {
            f.depth == 32
                && f.type_ == render::PictType::DIRECT
                && f.direct.alpha_mask == 0xff
                && f.direct.alpha_shift == 24
                && f.direct.red_shift == 16
                && f.direct.green_shift == 8
                && f.direct.blue_shift == 0
        })
        .map(|f| f.id)
}

/// Did the server accept our SHM segment? (It can't on remote displays.)
fn segment_works(conn: &RustConnection, root: Window, seg: u32) -> bool {
    // A 1×1 GetImage through the segment errors out if the attach failed.
    let Ok(cookie) = conn.shm_get_image(root, 0, 0, 1, 1, !0, ImageFormat::Z_PIXMAP.into(), seg, 0) else {
        return false;
    };
    cookie.reply().is_ok()
}

/// A 1×1 cursor with an empty mask.
fn invisible_cursor(conn: &RustConnection, root: Window) -> XResult<Cursor> {
    let pix = conn.generate_id().x11()?;
    conn.create_pixmap(1, pix, root, 1, 1).x11()?;
    let gc = conn.generate_id().x11()?;
    conn.create_gc(gc, pix, &CreateGCAux::new().foreground(0)).x11()?;
    conn.poly_fill_rectangle(pix, gc, &[Rectangle { x: 0, y: 0, width: 1, height: 1 }]).x11()?;
    let cursor = conn.generate_id().x11()?;
    conn.create_cursor(cursor, pix, pix, 0, 0, 0, 0, 0, 0, 0, 0).x11()?;
    conn.free_gc(gc).x11()?;
    conn.free_pixmap(pix).x11()?;
    Ok(cursor)
}

/// The opaque pixels (alpha > 127) of a w×h ARGB buffer as YX-banded rectangles: runs on
/// a row, with identical runs on consecutive rows merged into one band.
pub(super) fn alpha_rects(px: &[u32], w: usize, h: usize) -> Vec<Rectangle> {
    let mut rects = Vec::new();
    let mut prev: Vec<(u16, u16)> = Vec::new();
    let mut cur: Vec<(u16, u16)> = Vec::new();
    let mut band_start = 0;
    for y in 0..=h {
        cur.clear();
        if y < h {
            let row = &px[y * w..(y + 1) * w];
            let mut x = 0;
            while x < w {
                if row[x] >> 24 > 127 {
                    let s = x;
                    while x < w && row[x] >> 24 > 127 {
                        x += 1;
                    }
                    cur.push((s as u16, (x - s) as u16));
                } else {
                    x += 1;
                }
            }
        }
        if y == h || cur != prev {
            for &(x, len) in &prev {
                rects.push(Rectangle {
                    x: x as i16,
                    y: band_start as i16,
                    width: len,
                    height: (y - band_start) as u16,
                });
            }
            std::mem::swap(&mut prev, &mut cur);
            band_start = y;
        }
    }
    rects
}
