//! Taking and releasing control.
//!
//! Active capture is a full-screen layer surface with exclusive keyboard focus, a locked
//! pointer, and a shortcuts inhibitor. Every one of those is compositor-mediated, so if
//! this process dies the compositor tears them down and the desk keeps working. That is
//! the decisive advantage over grabbing evdev devices directly, where a wedged process
//! leaves the keyboard captured with no way back.

use wayland_client::protocol::wl_surface;
use wayland_protocols::wp::viewporter::client::wp_viewport;
use wayland_client::QueueHandle;
use wayland_protocols::wp::keyboard_shortcuts_inhibit::zv1::client::zwp_keyboard_shortcuts_inhibitor_v1 as ks_inhib;
use wayland_protocols::wp::pointer_constraints::zv1::client::{
    zwp_locked_pointer_v1 as locked_ptr, zwp_pointer_constraints_v1 as pc_mgr,
};
use wayland_protocols::wp::relative_pointer::zv1::client::zwp_relative_pointer_v1 as rel_ptr;
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1 as ls, zwlr_layer_surface_v1 as lsv,
};

use wayflow_proto::Msg;

use super::state::{Phase, State, SurfaceRole};

/// Live capture: the surface holding input plus the constraints keeping it here.
pub(crate) struct ActiveCapture {
    pub(crate) surface: wl_surface::WlSurface,
    pub(crate) layer_surface: lsv::ZwlrLayerSurfaceV1,
    pub(crate) inhibitor: ks_inhib::ZwpKeyboardShortcutsInhibitorV1,
    pub(crate) viewport: wp_viewport::WpViewport,
    /// Set once the overlay has been mapped with its transparent buffer.
    pub(crate) configured: bool,
    pub(crate) locked: Option<locked_ptr::ZwpLockedPointerV1>,
    pub(crate) relative: Option<rel_ptr::ZwpRelativePointerV1>,
    /// Horizontal distance travelled since the crossing, in unaccelerated device units.
    ///
    /// Starts at zero and goes negative moving into the client. Returning to zero means
    /// the pointer has walked all the way back to the edge it came from, which is the
    /// release condition. The client's width never enters into it, so the host does not
    /// need to know anything about the remote display.
    pub(crate) travel_x: f64,
    /// Keys currently held, so they can be released if capture ends mid-press.
    pub(crate) held_keys: Vec<u16>,
}

impl State {
    /// Take control. Called when the pointer reaches the edge strip.
    pub(crate) fn begin_capture(&mut self, edge_ratio: f64, qh: &QueueHandle<Self>) {
        if self.phase == Phase::Active {
            return;
        }
        if let Err(e) = self.sink.ensure_connected(&self.hostname) {
            // Not fatal: without a client there is nothing to hand control to, so stay
            // local rather than capturing input into a void.
            eprintln!("wayflow-host: no client on the tunnel ({e}); staying local");
            return;
        }

        let Some(idx) = self.boundary_output() else {
            return;
        };
        let output = self.outputs[idx].0.clone();
        let compositor = self.compositor.clone().expect("checked at startup");
        let shell = self.layer_shell.clone().expect("checked at startup");

        let surface = compositor.create_surface(qh, ());
        let layer_surface = shell.get_layer_surface(
            &surface,
            Some(&output),
            ls::Layer::Overlay,
            "wayflow-capture".to_owned(),
            qh,
            SurfaceRole::Overlay,
        );
        layer_surface.set_anchor(
            lsv::Anchor::Top | lsv::Anchor::Bottom | lsv::Anchor::Left | lsv::Anchor::Right,
        );
        layer_surface.set_size(0, 0);
        // -1 means "ignore other surfaces' exclusive zones", so the overlay covers the
        // bar as well. A gap there would let the pointer escape mid-capture.
        layer_surface.set_exclusive_zone(-1);
        layer_surface.set_keyboard_interactivity(lsv::KeyboardInteractivity::Exclusive);
        let viewport = self
            .viewporter
            .as_ref()
            .expect("checked at startup")
            .get_viewport(&surface, qh, ());
        surface.commit();

        let seat = self.seat.clone().expect("checked at startup");
        let inhibitor = self
            .ks_mgr
            .as_ref()
            .expect("checked at startup")
            .inhibit_shortcuts(&surface, &seat, qh, ());

        // Locking rather than confining: a locked pointer stops moving entirely, which is
        // what we want, since the cursor should appear frozen at the edge while the user
        // is driving the other machine.
        let locked = if let (Some(pc), Some(ptr)) = (&self.pc_mgr, &self.pointer) {
            Some(pc.lock_pointer(&surface, ptr, None, pc_mgr::Lifetime::Persistent, qh, ()))
        } else {
            eprintln!("wayflow-host: no pointer constraints; cursor will move locally");
            None
        };

        // Relative motion keeps arriving even though the pointer is pinned, which is the
        // only reason a locked cursor still produces usable deltas.
        let relative = if let (Some(mgr), Some(ptr)) = (&self.rel_ptr_mgr, &self.pointer) {
            Some(mgr.get_relative_pointer(ptr, qh, ()))
        } else {
            None
        };

        self.active = Some(ActiveCapture {
            surface,
            layer_surface,
            viewport,
            configured: false,
            inhibitor,
            locked,
            relative,
            travel_x: 0.0,
            held_keys: Vec::new(),
        });
        self.phase = Phase::Active;
        self.sink.send(&Msg::Enter { edge_ratio });
        eprintln!("wayflow-host: control -> client (edge ratio {edge_ratio:.2})");
    }

    /// Hand control back to this machine.
    pub(crate) fn end_capture(&mut self, qh: &QueueHandle<Self>) {
        let Some(active) = self.active.take() else {
            self.phase = Phase::Idle;
            return;
        };

        // Release anything still held before the client stops receiving events, or it
        // will believe those keys are down indefinitely.
        for code in &active.held_keys {
            self.sink.send_input(wayflow_proto::Input::Key {
                code: *code,
                pressed: false,
            });
        }
        self.sink.send(&Msg::Leave);

        if let Some(rel) = active.relative {
            rel.destroy();
        }
        if let Some(locked) = active.locked {
            locked.destroy();
        }
        active.inhibitor.destroy();
        active.viewport.destroy();
        active.layer_surface.destroy();
        active.surface.destroy();

        self.phase = Phase::Idle;
        self.nudge_cursor_inward(qh);
        eprintln!("wayflow-host: control -> local");
    }

    /// Push the cursor away from the edge on release.
    ///
    /// Without this the pointer is left sitting exactly on the strip, which immediately
    /// re-triggers a crossing and control ping-pongs between machines.
    fn nudge_cursor_inward(&self, qh: &QueueHandle<Self>) {
        let Some(mgr) = &self.vp_mgr else { return };
        let pointer = mgr.create_virtual_pointer(self.seat.as_ref(), qh, ());
        let (dx, dy) = match self.config.edge {
            super::Edge::Left => (NUDGE, 0.0),
            super::Edge::Right => (-NUDGE, 0.0),
            super::Edge::Top => (0.0, NUDGE),
            super::Edge::Bottom => (0.0, -NUDGE),
        };
        pointer.motion(0, dx, dy);
        pointer.frame();
        pointer.destroy();
    }
}

/// Far enough that the pointer clears the one-pixel strip and any rounding around it.
const NUDGE: f64 = 12.0;



