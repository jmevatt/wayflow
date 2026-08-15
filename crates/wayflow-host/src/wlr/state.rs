//! Capture state and the globals it is assembled from.

use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_keyboard, wl_output, wl_pointer, wl_seat, wl_surface,
};
use wayland_client::QueueHandle;
use wayland_protocols::wp::keyboard_shortcuts_inhibit::zv1::client::zwp_keyboard_shortcuts_inhibit_manager_v1 as ks_mgr;
use wayland_protocols::wp::pointer_constraints::zv1::client::zwp_pointer_constraints_v1 as pc_mgr;
use wayland_protocols::wp::relative_pointer::zv1::client::zwp_relative_pointer_manager_v1 as rel_ptr_mgr;
use wayland_protocols::wp::single_pixel_buffer::v1::client::wp_single_pixel_buffer_manager_v1 as spbuf;
use wayland_protocols::wp::viewporter::client::{wp_viewport, wp_viewporter};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1 as ls, zwlr_layer_surface_v1 as lsv,
};
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_manager_v1 as vp_mgr;

use super::capture::ActiveCapture;
use super::{Config, Error};
use crate::sink::Sink;

/// The screen edge that leads to the remote machine.
///
/// All four are load-bearing even though only `Left` is reachable today: the placement
/// and anchor logic below is written against the general case, and narrowing the enum to
/// what a not-yet-written CLI flag can select would mean rewriting it later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

impl Edge {
    /// Anchors pinning a strip to this edge and stretching it along the perpendicular.
    pub(crate) fn anchors(self) -> lsv::Anchor {
        use lsv::Anchor;
        match self {
            Self::Left => Anchor::Left | Anchor::Top | Anchor::Bottom,
            Self::Right => Anchor::Right | Anchor::Top | Anchor::Bottom,
            Self::Top => Anchor::Top | Anchor::Left | Anchor::Right,
            Self::Bottom => Anchor::Bottom | Anchor::Left | Anchor::Right,
        }
    }

    /// A zero dimension tells the compositor to stretch that axis to the output, so a
    /// vertical strip is one pixel wide and full height.
    pub(crate) fn strip_size(self) -> (u32, u32) {
        match self {
            Self::Left | Self::Right => (1, 0),
            Self::Top | Self::Bottom => (0, 1),
        }
    }

}

/// `Active` is unreachable until the capture overlay lands; the idle half of the state
/// machine is what edge detection needs, and splitting the enum in two to avoid a
/// temporarily-unused variant would just have to be undone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Phase {
    Idle,
    Active,
}

/// Which surface a configure event belongs to. Both the strip and the overlay are layer
/// surfaces, and they need opposite handling, so the role travels as user data rather
/// than being guessed from the event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceRole {
    Strip,
    Overlay,
}

/// Geometry for one monitor, accumulated across `wl_output` events.
#[derive(Debug, Default, Clone)]
pub struct OutputInfo {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    /// `wl_output` reports position and mode separately; only after `done` is the
    /// geometry coherent enough to place a surface against.
    pub done: bool,
}

pub(crate) struct EdgeStrip {
    pub(crate) surface: wl_surface::WlSurface,
    /// Held rather than read: teardown calls `destroy` on it when capture ends, and
    /// there is nothing to read from a layer surface in the meantime.
    #[allow(dead_code)]
    pub(crate) layer_surface: lsv::ZwlrLayerSurfaceV1,
    pub(crate) viewport: wp_viewport::WpViewport,
    /// Set once the first configure has been acknowledged and a buffer attached. The
    /// compositor re-sends configure on output changes, and re-attaching each time would
    /// pointlessly remap a surface that is already correct.
    pub(crate) configured: bool,
}

pub struct State {
    pub(crate) config: Config,
    pub(crate) phase: Phase,

    pub(crate) compositor: Option<wl_compositor::WlCompositor>,
    pub(crate) layer_shell: Option<ls::ZwlrLayerShellV1>,
    pub(crate) viewporter: Option<wp_viewporter::WpViewporter>,
    pub(crate) spbuf_mgr: Option<spbuf::WpSinglePixelBufferManagerV1>,
    pub(crate) seat: Option<wl_seat::WlSeat>,
    pub(crate) pointer: Option<wl_pointer::WlPointer>,
    pub(crate) keyboard: Option<wl_keyboard::WlKeyboard>,
    pub(crate) pc_mgr: Option<pc_mgr::ZwpPointerConstraintsV1>,
    pub(crate) rel_ptr_mgr: Option<rel_ptr_mgr::ZwpRelativePointerManagerV1>,
    pub(crate) ks_mgr: Option<ks_mgr::ZwpKeyboardShortcutsInhibitManagerV1>,
    pub(crate) vp_mgr: Option<vp_mgr::ZwlrVirtualPointerManagerV1>,

    pub(crate) active: Option<ActiveCapture>,
    pub(crate) sink: Sink,
    pub(crate) hostname: String,

    pub(crate) outputs: Vec<(wl_output::WlOutput, OutputInfo)>,
    pub(crate) strip: Option<EdgeStrip>,
    /// One transparent pixel, scaled by a viewport to whatever size each surface needs.
    /// A layer surface stays unmapped until it has a buffer, so even an invisible surface
    /// needs one.
    pub(crate) blank: Option<wl_buffer::WlBuffer>,
}

impl State {
    #[must_use]
    pub fn new(config: Config, port: u16) -> Self {
        Self {
            config,
            phase: Phase::Idle,
            compositor: None,
            layer_shell: None,
            viewporter: None,
            spbuf_mgr: None,
            seat: None,
            pointer: None,
            keyboard: None,
            pc_mgr: None,
            rel_ptr_mgr: None,
            ks_mgr: None,
            vp_mgr: None,
            active: None,
            sink: Sink::new(port),
            hostname: read_hostname(),
            outputs: Vec::new(),
            strip: None,
            blank: None,
        }
    }

    /// Fail on a missing protocol at startup rather than at the moment it is needed.
    ///
    /// # Errors
    /// Names the first absent global.
    pub fn check_globals(&self) -> Result<(), Error> {
        if self.compositor.is_none() {
            return Err(Error::MissingGlobal("wl_compositor"));
        }
        if self.layer_shell.is_none() {
            return Err(Error::MissingGlobal("zwlr_layer_shell_v1"));
        }
        if self.viewporter.is_none() {
            return Err(Error::MissingGlobal("wp_viewporter"));
        }
        if self.spbuf_mgr.is_none() {
            return Err(Error::MissingGlobal("wp_single_pixel_buffer_manager_v1"));
        }
        if self.seat.is_none() {
            return Err(Error::MissingGlobal("wl_seat"));
        }
        if self.ks_mgr.is_none() {
            return Err(Error::MissingGlobal(
                "zwp_keyboard_shortcuts_inhibit_manager_v1",
            ));
        }
        if self.rel_ptr_mgr.is_none() {
            return Err(Error::MissingGlobal("zwp_relative_pointer_manager_v1"));
        }
        Ok(())
    }

    #[must_use]
    pub fn strip_placed(&self) -> bool {
        self.strip.is_some()
    }

    /// Height of the strip, for turning a surface-local y into a fraction of the edge.
    ///
    /// Taken from the output rather than the granted surface size because the ratio
    /// should describe position on the monitor, not on whatever sliver of it sway left
    /// after subtracting exclusive zones.
    pub(crate) fn strip_height(&self) -> f64 {
        self.boundary_output()
            .map_or(1.0, |i| f64::from(self.outputs[i].1.height))
    }

    /// A fully transparent one-pixel buffer, created once and shared.
    pub(crate) fn blank_buffer(&mut self, qh: &QueueHandle<Self>) -> wl_buffer::WlBuffer {
        if let Some(b) = &self.blank {
            return b.clone();
        }
        let mgr = self.spbuf_mgr.as_ref().expect("checked in check_globals");
        let buffer = mgr.create_u32_rgba_buffer(0, 0, 0, 0, qh, ());
        self.blank = Some(buffer.clone());
        buffer
    }

    /// The output whose edge is the outer boundary of the desktop on the configured side.
    ///
    /// With three monitors side by side, only the leftmost one's left edge actually leaves
    /// the desktop; a strip on the others would fire in the middle of the screen.
    pub(crate) fn boundary_output(&self) -> Option<usize> {
        let ready: Vec<usize> = (0..self.outputs.len())
            .filter(|&i| self.outputs[i].1.done)
            .collect();
        let key = |i: &usize| -> i32 {
            let g = &self.outputs[*i].1;
            match self.config.edge {
                Edge::Left => g.x,
                Edge::Right => -(g.x + g.width),
                Edge::Top => g.y,
                Edge::Bottom => -(g.y + g.height),
            }
        };
        // Negating the far-side coordinates above lets a single minimisation serve all
        // four edges.
        ready.into_iter().min_by_key(|i| key(i))
    }
}

fn read_hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map_or_else(|_| "unknown".to_owned(), |s| s.trim().to_owned())
}
