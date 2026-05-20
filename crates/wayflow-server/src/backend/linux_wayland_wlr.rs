// Input capture via wlroots protocols: zwlr_layer_shell_v1 for edge detection and
// input grab. Used when the XDG InputCapture portal is unavailable (e.g. Sway).
//
// Phase 1 (IDLE):
//   1-pixel edge strips on all four sides of each wl_output (LAYER_OVERLAY, no
//   keyboard interaction). wl_pointer::enter on any strip → Phase 2.
//
// Phase 2 (ACTIVE):
//   Full-screen LAYER_OVERLAY with keyboard_interactivity=EXCLUSIVE.
//   zwp_keyboard_shortcuts_inhibit_manager captures all key combos.
//   wl_pointer + wl_keyboard events are forwarded as InputEvent values.
//   On release_rx: nudge cursor via zwlr_virtual_pointer_v1, then Phase 1.

use std::{collections::HashSet, os::fd::AsFd, os::unix::io::AsRawFd, sync::Arc};

use anyhow::{Context, Result};
use tokio::io::unix::AsyncFd;
use tokio::sync::{mpsc, watch};
use tracing::{debug, info, warn};
use wayland_client::{
    delegate_noop,
    protocol::{
        wl_buffer, wl_compositor, wl_keyboard, wl_output, wl_pointer, wl_registry, wl_seat,
        wl_surface,
    },
    Connection, Dispatch, QueueHandle, WEnum,
};
use wayland_protocols::wp::{
    keyboard_shortcuts_inhibit::zv1::client::{
        zwp_keyboard_shortcuts_inhibit_manager_v1 as ks_mgr,
        zwp_keyboard_shortcuts_inhibitor_v1 as ks_inhib,
    },
    pointer_constraints::zv1::client::{
        zwp_locked_pointer_v1 as locked_ptr,
        zwp_pointer_constraints_v1 as pc_mgr,
    },
    relative_pointer::zv1::client::{
        zwp_relative_pointer_manager_v1 as rel_ptr_mgr,
        zwp_relative_pointer_v1 as rel_ptr,
    },
    single_pixel_buffer::v1::client::wp_single_pixel_buffer_manager_v1 as spbuf,
    viewporter::client::{wp_viewport, wp_viewporter},
};
use wayland_protocols_wlr::{
    layer_shell::v1::client::{zwlr_layer_shell_v1 as ls, zwlr_layer_surface_v1 as lsv},
    virtual_pointer::v1::client::{
        zwlr_virtual_pointer_manager_v1 as vp_mgr, zwlr_virtual_pointer_v1 as vp,
    },
};
use wayflow_proto::{Modifiers, ScreenInfo};

use super::linux_wayland::{evdev_to_mouse_button, try_emit, xkb_mods_to_proto};
use super::InputEvent;
use crate::telemetry::Telemetry;

// ── Types ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Active,
}

/// User-data tag attached to each ZwlrLayerSurfaceV1 so configure events
/// can identify whether the surface is an edge strip or the full-screen overlay.
#[derive(Debug, Clone, Copy)]
enum SurfaceRole {
    EdgeStrip(Edge, usize), // which edge, output index
    Overlay,
}

struct OutputInfo {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    done: bool,
}

struct EdgeStrip {
    surface: wl_surface::WlSurface,
    layer_surface: lsv::ZwlrLayerSurfaceV1,
    viewport: wp_viewport::WpViewport,
    edge: Edge,
    output_idx: usize,
}

struct ActiveCapture {
    surface: wl_surface::WlSurface,
    layer_surface: lsv::ZwlrLayerSurfaceV1,
    viewport: wp_viewport::WpViewport,
    inhibitor: ks_inhib::ZwpKeyboardShortcutsInhibitorV1,
    locked_pointer: Option<locked_ptr::ZwpLockedPointerV1>,
    activation_edge: Edge,
    output_idx: usize,
    /// Real screen position from wl_pointer (clamped). Used for cursor nudge on release.
    cursor: (f64, f64),
    /// Unbounded relative accumulator forwarded to route_events via MouseMoveAbs.
    /// Updated by zwp_relative_pointer_v1 deltas so dx is non-zero past screen edges.
    virtual_cursor: (f64, f64),
}

struct State {
    // Globals (populated during registry roundtrip)
    compositor: Option<wl_compositor::WlCompositor>,
    seat: Option<wl_seat::WlSeat>,
    layer_shell: Option<ls::ZwlrLayerShellV1>,
    vp_mgr: Option<vp_mgr::ZwlrVirtualPointerManagerV1>,
    ks_mgr: Option<ks_mgr::ZwpKeyboardShortcutsInhibitManagerV1>,
    pc_mgr: Option<pc_mgr::ZwpPointerConstraintsV1>,
    spbuf_mgr: Option<spbuf::WpSinglePixelBufferManagerV1>,
    viewporter: Option<wp_viewporter::WpViewporter>,

    // Outputs (one entry per wl_output, populated from output events)
    outputs: Vec<OutputInfo>,
    wl_outputs: Vec<wl_output::WlOutput>,

    // Input devices (bound once seat capabilities arrive)
    pointer: Option<wl_pointer::WlPointer>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    rel_ptr_mgr: Option<rel_ptr_mgr::ZwpRelativePointerManagerV1>,
    relative_pointer: Option<rel_ptr::ZwpRelativePointerV1>,

    // 1×1 transparent ARGB buffer shared across all surfaces
    shared_buffer: Option<wl_buffer::WlBuffer>,

    // Virtual pointer used to nudge the real cursor on release
    virtual_pointer: Option<vp::ZwlrVirtualPointerV1>,

    // State machine
    phase: Phase,
    edge_strips: Vec<EdgeStrip>,
    active: Option<ActiveCapture>,

    // Set inside Dispatch impl when cursor enters an edge strip; consumed after dispatch.
    pending_activation: Option<(Edge, usize, (f64, f64))>,
    // After pending_activation emits MouseMoveAbs we park here for one select! tick.
    // If route_events rejects the edge (no client → immediate release_rx), we drop it
    // silently instead of creating an overlay that flickers and nudges the cursor.
    deferred_activation: Option<(Edge, usize, (f64, f64))>,

    // Input tracking
    xkb_mods: Modifiers,
    held_keys: HashSet<u32>,

    // Scroll accumulator (axis events are batched per frame)
    scroll_acc: (f64, f64),

    // Event forwarding
    tx: mpsc::Sender<InputEvent>,
    monitors_tx: watch::Sender<Vec<ScreenInfo>>,
    telemetry: Arc<Telemetry>,
}

// ── State helpers ─────────────────────────────────────────────────────────────

impl State {
    fn publish_monitors(&self) {
        let screens: Vec<ScreenInfo> = self
            .outputs
            .iter()
            .enumerate()
            .map(|(i, o)| ScreenInfo {
                name: format!("output-{i}"),
                x: o.x,
                y: o.y,
                width: o.width as u16,
                height: o.height as u16,
            })
            .collect();
        if !screens.is_empty() {
            let _ = self.monitors_tx.send(screens);
        }
    }

    /// Ensure the shared transparent buffer exists and return a clone.
    fn get_buffer(&mut self, qh: &QueueHandle<State>) -> wl_buffer::WlBuffer {
        if self.shared_buffer.is_none() {
            let mgr = self.spbuf_mgr.as_ref().expect("wp_single_pixel_buffer_manager_v1");
            self.shared_buffer = Some(mgr.create_u32_rgba_buffer(0, 0, 0, 0, qh, ()));
        }
        self.shared_buffer.clone().unwrap()
    }

    #[allow(clippy::too_many_arguments)]
    fn create_layer_surface(
        &mut self,
        output: &wl_output::WlOutput,
        layer: ls::Layer,
        anchor: lsv::Anchor,
        (w, h): (u32, u32),
        excl_zone: i32,
        ki: lsv::KeyboardInteractivity,
        role: SurfaceRole,
        qh: &QueueHandle<State>,
    ) -> (wl_surface::WlSurface, lsv::ZwlrLayerSurfaceV1, wp_viewport::WpViewport) {
        let comp = self.compositor.as_ref().expect("wl_compositor");
        let shell = self.layer_shell.as_ref().expect("zwlr_layer_shell_v1");
        let vp = self.viewporter.as_ref().expect("wp_viewporter");

        let surface = comp.create_surface(qh, ());
        let layer_surface =
            shell.get_layer_surface(&surface, Some(output), layer, "wayflow".into(), qh, role);

        layer_surface.set_anchor(anchor);
        layer_surface.set_size(w, h);
        layer_surface.set_exclusive_zone(excl_zone);
        layer_surface.set_keyboard_interactivity(ki);

        let viewport = vp.get_viewport(&surface, qh, ());
        // Initial commit to request a configure event from the compositor.
        surface.commit();

        (surface, layer_surface, viewport)
    }

    /// Caller provides (output_idx, WlOutput) pairs.
    fn create_strips_for_output(
        &mut self,
        output_idx: usize,
        wl_out: &wl_output::WlOutput,
        qh: &QueueHandle<State>,
    ) {
        use lsv::{Anchor, KeyboardInteractivity};
        use ls::Layer;

        let edges = [
            (Edge::Right,  Anchor::Right  | Anchor::Top | Anchor::Bottom, (1, 0)),
            (Edge::Left,   Anchor::Left   | Anchor::Top | Anchor::Bottom, (1, 0)),
            (Edge::Top,    Anchor::Top    | Anchor::Left | Anchor::Right, (0, 1)),
            (Edge::Bottom, Anchor::Bottom | Anchor::Left | Anchor::Right, (0, 1)),
        ];

        for (edge, anchor, size) in edges {
            let (surface, layer_surface, viewport) = self.create_layer_surface(
                wl_out,
                Layer::Overlay,
                anchor,
                size,
                0,
                KeyboardInteractivity::None,
                SurfaceRole::EdgeStrip(edge, output_idx),
                qh,
            );
            self.edge_strips.push(EdgeStrip { surface, layer_surface, viewport, edge, output_idx });
        }
    }

    fn destroy_edge_strips(&mut self) {
        for strip in self.edge_strips.drain(..) {
            strip.layer_surface.destroy();
            strip.viewport.destroy();
            strip.surface.destroy();
        }
    }

    fn create_overlay(
        &mut self,
        activation_edge: Edge,
        output_idx: usize,
        cursor_pos: (f64, f64),
        wl_out: &wl_output::WlOutput,
        qh: &QueueHandle<State>,
    ) {
        use lsv::{Anchor, KeyboardInteractivity};
        use ls::Layer;

        let anchor = Anchor::Left | Anchor::Right | Anchor::Top | Anchor::Bottom;
        let (surface, layer_surface, viewport) = self.create_layer_surface(
            wl_out,
            Layer::Overlay,
            anchor,
            (0, 0),
            -1,
            KeyboardInteractivity::Exclusive,
            SurfaceRole::Overlay,
            qh,
        );

        let seat = self.seat.as_ref().expect("wl_seat");
        let ks_mgr = self.ks_mgr.as_ref().expect("zwp_keyboard_shortcuts_inhibit_manager_v1");
        let inhibitor = ks_mgr.inhibit_shortcuts(&surface, seat, qh, ());

        let locked_pointer = match (&self.pc_mgr, &self.pointer) {
            (Some(pc), Some(ptr)) => Some(pc.lock_pointer(
                &surface,
                ptr,
                None,
                pc_mgr::Lifetime::Persistent,
                qh,
                (),
            )),
            _ => {
                warn!("zwp_pointer_constraints_v1 unavailable — cursor will move on both screens");
                None
            }
        };

        self.active = Some(ActiveCapture {
            surface,
            layer_surface,
            viewport,
            inhibitor,
            locked_pointer,
            activation_edge,
            output_idx,
            cursor: cursor_pos,
            virtual_cursor: cursor_pos,
        });
        self.phase = Phase::Active;
        info!("wlr capture activated on {:?} edge, output {output_idx}", activation_edge);
    }

    fn destroy_overlay(&mut self) {
        if let Some(cap) = self.active.take() {
            if let Some(lp) = cap.locked_pointer { lp.destroy(); }
            cap.inhibitor.destroy();
            cap.layer_surface.destroy();
            cap.viewport.destroy();
            cap.surface.destroy();
        }
        self.phase = Phase::Idle;
    }

    /// Nudge cursor 5px away from the activation edge using a virtual pointer,
    /// so the real cursor lands back inside the local screen on release.
    fn nudge_cursor_on_release(&self, output_idx: usize) {
        let vp = match &self.virtual_pointer {
            Some(v) => v,
            None => return,
        };
        let cap = match &self.active {
            Some(c) => c,
            None => return,
        };
        let out = match self.outputs.get(output_idx) {
            Some(o) => o,
            None => return,
        };

        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u32)
            .unwrap_or(0);

        let (mut x, mut y) = cap.cursor;
        const NUDGE: f64 = 5.0;
        match cap.activation_edge {
            Edge::Right  => x = (out.x + out.width)  as f64 - NUDGE,
            Edge::Left   => x =  out.x                as f64 + NUDGE,
            Edge::Bottom => y = (out.y + out.height)  as f64 - NUDGE,
            Edge::Top    => y =  out.y                as f64 + NUDGE,
        }

        vp.motion_absolute(
            ms,
            x as u32,
            y as u32,
            out.width  as u32,
            out.height as u32,
        );
        vp.frame();
    }

    /// Called when release_rx fires. Nudges cursor, tears down overlay, rebuilds strips.
    fn handle_release(&mut self, outputs_with_wl: &[(usize, wl_output::WlOutput)], qh: &QueueHandle<State>) {
        // Synthesise key-releases for everything held so the client doesn't get stuck keys.
        let mods = Modifiers::default();
        for hid in self.held_keys.drain().collect::<Vec<_>>() {
            try_emit(&self.tx, InputEvent::Key { keycode: hid, pressed: false, modifiers: mods }, &self.telemetry);
        }

        let output_idx = self.active.as_ref().map(|c| c.output_idx).unwrap_or(0);
        self.nudge_cursor_on_release(output_idx);
        self.destroy_overlay();

        for (idx, wl_out) in outputs_with_wl {
            self.create_strips_for_output(*idx, wl_out, qh);
        }
        info!("wlr capture released; edge strips rebuilt");
    }
}

// ── Dispatch implementations ──────────────────────────────────────────────────

// Interfaces with no events we need to handle.
delegate_noop!(State: ignore wl_compositor::WlCompositor);
delegate_noop!(State: ignore wl_surface::WlSurface);
delegate_noop!(State: ignore wl_buffer::WlBuffer);
delegate_noop!(State: ignore ls::ZwlrLayerShellV1);
delegate_noop!(State: ignore vp_mgr::ZwlrVirtualPointerManagerV1);
delegate_noop!(State: ignore vp::ZwlrVirtualPointerV1);
delegate_noop!(State: ignore spbuf::WpSinglePixelBufferManagerV1);
delegate_noop!(State: ignore wp_viewporter::WpViewporter);
delegate_noop!(State: ignore wp_viewport::WpViewport);
delegate_noop!(State: ignore ks_mgr::ZwpKeyboardShortcutsInhibitManagerV1);
delegate_noop!(State: ignore ks_inhib::ZwpKeyboardShortcutsInhibitorV1);
delegate_noop!(State: ignore pc_mgr::ZwpPointerConstraintsV1);
delegate_noop!(State: ignore locked_ptr::ZwpLockedPointerV1);
delegate_noop!(State: ignore rel_ptr_mgr::ZwpRelativePointerManagerV1);

// wl_registry — bind globals as they are advertised.
impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_registry::Event::Global { name, interface, version } = event else { return };
        match interface.as_str() {
            "wl_compositor" => {
                state.compositor =
                    Some(registry.bind::<wl_compositor::WlCompositor, _, _>(name, version.min(6), qh, ()));
            }
            "wl_seat" => {
                state.seat =
                    Some(registry.bind::<wl_seat::WlSeat, _, _>(name, version.min(9), qh, ()));
            }
            "wl_output" => {
                let idx = state.outputs.len();
                let wl_out = registry.bind::<wl_output::WlOutput, _, _>(name, version.min(4), qh, idx);
                state.wl_outputs.push(wl_out);
                state.outputs.push(OutputInfo { x: 0, y: 0, width: 0, height: 0, done: false });
            }
            "zwlr_layer_shell_v1" => {
                state.layer_shell =
                    Some(registry.bind::<ls::ZwlrLayerShellV1, _, _>(name, version.min(4), qh, ()));
            }
            "zwlr_virtual_pointer_manager_v1" => {
                state.vp_mgr =
                    Some(registry.bind::<vp_mgr::ZwlrVirtualPointerManagerV1, _, _>(name, version.min(2), qh, ()));
            }
            "zwp_keyboard_shortcuts_inhibit_manager_v1" => {
                state.ks_mgr =
                    Some(registry.bind::<ks_mgr::ZwpKeyboardShortcutsInhibitManagerV1, _, _>(name, 1, qh, ()));
            }
            "zwp_pointer_constraints_v1" => {
                state.pc_mgr =
                    Some(registry.bind::<pc_mgr::ZwpPointerConstraintsV1, _, _>(name, 1, qh, ()));
            }
            "wp_single_pixel_buffer_manager_v1" => {
                state.spbuf_mgr =
                    Some(registry.bind::<spbuf::WpSinglePixelBufferManagerV1, _, _>(name, 1, qh, ()));
            }
            "wp_viewporter" => {
                state.viewporter =
                    Some(registry.bind::<wp_viewporter::WpViewporter, _, _>(name, 1, qh, ()));
            }
            "zwp_relative_pointer_manager_v1" => {
                state.rel_ptr_mgr =
                    Some(registry.bind::<rel_ptr_mgr::ZwpRelativePointerManagerV1, _, _>(name, 1, qh, ()));
            }
            _ => {}
        }
    }
}

// wl_output — capture screen geometry.
impl Dispatch<wl_output::WlOutput, usize> for State {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        idx: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let idx = *idx;
        let Some(out) = state.outputs.get_mut(idx) else { return };
        match event {
            wl_output::Event::Geometry { x, y, .. } => {
                out.x = x;
                out.y = y;
            }
            wl_output::Event::Mode { width, height, flags: WEnum::Value(f), .. }
                if f.contains(wl_output::Mode::Current) =>
            {
                out.width = width;
                out.height = height;
            }
            wl_output::Event::Done => {
                out.done = true;
                if state.outputs.iter().all(|o| o.done) {
                    state.publish_monitors();
                }
            }
            _ => {}
        }
    }
}

// wl_seat — bind pointer and keyboard when capabilities arrive.
impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities { capabilities: WEnum::Value(caps) } = event {
            if caps.contains(wl_seat::Capability::Pointer) && state.pointer.is_none() {
                let ptr = seat.get_pointer(qh, ());
                if let Some(ref mgr) = state.rel_ptr_mgr.clone() {
                    state.relative_pointer = Some(mgr.get_relative_pointer(&ptr, qh, ()));
                }
                state.pointer = Some(ptr);
            }
            if caps.contains(wl_seat::Capability::Keyboard) && state.keyboard.is_none() {
                state.keyboard = Some(seat.get_keyboard(qh, ()));
            }
            // Create virtual pointer for cursor nudge.
            if state.virtual_pointer.is_none() {
                if let Some(ref mgr) = state.vp_mgr.clone() {
                    state.virtual_pointer = Some(mgr.create_virtual_pointer(None, qh, ()));
                }
            }
        }
    }
}

// wl_pointer — edge detection (Phase 1) and event forwarding (Phase 2).
impl Dispatch<wl_pointer::WlPointer, ()> for State {
    fn event(
        state: &mut Self,
        _pointer: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter { surface, surface_x, surface_y, .. } => {
                match state.phase {
                    Phase::Idle => {
                        // Find the edge strip this surface belongs to.
                        if let Some(strip) = state.edge_strips.iter().find(|s| s.surface == surface) {
                            let edge = strip.edge;
                            let output_idx = strip.output_idx;
                            let cursor = if let Some(out) = state.outputs.get(output_idx) {
                                match edge {
                                    Edge::Right  => ((out.x + out.width - 1) as f64, out.y as f64 + surface_y),
                                    Edge::Left   => ( out.x                  as f64, out.y as f64 + surface_y),
                                    Edge::Top    => ( out.x as f64 + surface_x,       out.y          as f64),
                                    Edge::Bottom => ( out.x as f64 + surface_x,      (out.y + out.height - 1) as f64),
                                }
                            } else {
                                (surface_x, surface_y)
                            };
                            debug!("edge strip entered: {:?} out={output_idx} pos={cursor:?}", edge);
                            // Deferred: create_overlay borrows state.outputs and state.layer_shell,
                            // which we can't do while Dispatch holds a reference. Store the intent
                            // here; the main loop acts on it after dispatch_pending returns.
                            state.pending_activation = Some((edge, output_idx, cursor));
                        }
                    }
                    Phase::Active => {
                        // In active phase, the pointer entered the full-screen overlay.
                        if let Some(ref cap) = state.active {
                            if cap.surface == surface {
                                // Pointer is now over our overlay — update cursor position.
                                // surface_x/surface_y are overlay-relative = screen-relative
                                // (overlay is at 0,0 full screen).
                                if let Some(out) = state.outputs.get(cap.output_idx) {
                                    let abs_x = out.x as f64 + surface_x;
                                    let abs_y = out.y as f64 + surface_y;
                                    state.active.as_mut().unwrap().cursor = (abs_x, abs_y);
                                }
                            }
                        }
                    }
                }
            }

            wl_pointer::Event::Motion { surface_x, surface_y, .. } if state.phase == Phase::Active => {
                // Update real cursor position for nudge-on-release; do NOT emit MouseMoveAbs here.
                // MouseMoveAbs is driven by zwp_relative_pointer_v1 to avoid clamping at edges.
                if let Some(ref cap) = state.active {
                    let out_x = state.outputs.get(cap.output_idx).map(|o| o.x).unwrap_or(0);
                    let out_y = state.outputs.get(cap.output_idx).map(|o| o.y).unwrap_or(0);
                    let abs_x = out_x as f64 + surface_x;
                    let abs_y = out_y as f64 + surface_y;
                    state.active.as_mut().unwrap().cursor = (abs_x, abs_y);
                }
            }

            wl_pointer::Event::Button { button, state: WEnum::Value(s), .. }
                if state.phase == Phase::Active =>
            {
                let pressed = s == wl_pointer::ButtonState::Pressed;
                let btn = evdev_to_mouse_button(button);
                try_emit(&state.tx, InputEvent::MouseButton { button: btn, pressed }, &state.telemetry);
            }

            wl_pointer::Event::Axis { axis: WEnum::Value(ax), value, .. }
                if state.phase == Phase::Active =>
            {
                const PIXELS_PER_CLICK: f64 = 10.0;
                let delta = value / PIXELS_PER_CLICK;
                match ax {
                    wl_pointer::Axis::HorizontalScroll => state.scroll_acc.0 += delta,
                    wl_pointer::Axis::VerticalScroll   => state.scroll_acc.1 += delta,
                    _ => {}
                }
            }

            wl_pointer::Event::AxisDiscrete { axis: WEnum::Value(ax), discrete }
                if state.phase == Phase::Active =>
            {
                match ax {
                    wl_pointer::Axis::HorizontalScroll => state.scroll_acc.0 += discrete as f64,
                    wl_pointer::Axis::VerticalScroll   => state.scroll_acc.1 += discrete as f64,
                    _ => {}
                }
            }

            wl_pointer::Event::Frame if state.phase == Phase::Active => {
                let ix = state.scroll_acc.0.trunc();
                let iy = state.scroll_acc.1.trunc();
                state.scroll_acc.0 -= ix;
                state.scroll_acc.1 -= iy;
                if ix != 0.0 || iy != 0.0 {
                    try_emit(&state.tx, InputEvent::Scroll { dx: ix, dy: iy }, &state.telemetry);
                }
            }

            _ => {}
        }
    }
}

// wl_keyboard — key forwarding in Phase 2.
impl Dispatch<wl_keyboard::WlKeyboard, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Key { key, state: WEnum::Value(ks), .. }
                if state.phase == Phase::Active =>
            {
                let pressed = ks == wl_keyboard::KeyState::Pressed;
                if let Some(hid) = wayflow_core::keymap::evdev::evdev_to_hid(key) {
                    if pressed { state.held_keys.insert(hid); } else { state.held_keys.remove(&hid); }
                    try_emit(
                        &state.tx,
                        InputEvent::Key { keycode: hid, pressed, modifiers: state.xkb_mods },
                        &state.telemetry,
                    );
                } else {
                    debug!("evdev key {key} has no HID mapping, skipping");
                }
            }

            wl_keyboard::Event::Modifiers { mods_depressed, .. } => {
                state.xkb_mods = xkb_mods_to_proto(mods_depressed);
            }

            _ => {}
        }
    }
}

// zwp_relative_pointer_v1 — unclamped relative deltas during Phase 2.
// Drives virtual_cursor past screen edges so route_events sees non-zero dx/dy.
impl Dispatch<rel_ptr::ZwpRelativePointerV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &rel_ptr::ZwpRelativePointerV1,
        event: rel_ptr::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if state.phase != Phase::Active { return; }
        let rel_ptr::Event::RelativeMotion { dx, dy, .. } = event else { return };
        let Some(cap) = state.active.as_mut() else { return };
        cap.virtual_cursor.0 += dx;
        cap.virtual_cursor.1 += dy;
        let (vx, vy) = cap.virtual_cursor;
        try_emit(&state.tx, InputEvent::MouseMoveAbs { x: vx, y: vy }, &state.telemetry);
    }
}

// zwlr_layer_surface_v1 — respond to configure, attach buffer on first configure.
impl Dispatch<lsv::ZwlrLayerSurfaceV1, SurfaceRole> for State {
    fn event(
        state: &mut Self,
        layer_surface: &lsv::ZwlrLayerSurfaceV1,
        event: lsv::Event,
        role: &SurfaceRole,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let lsv::Event::Configure { serial, width, height } = event else { return };
        layer_surface.ack_configure(serial);

        // Find the matching surface object so we can attach the buffer + set viewport.
        let Some((surface, viewport)) = (match role {
            SurfaceRole::EdgeStrip(edge, output_idx) => {
                state.edge_strips
                    .iter()
                    .find(|s| s.edge == *edge && s.output_idx == *output_idx)
                    .map(|s| (s.surface.clone(), s.viewport.clone()))
            }
            SurfaceRole::Overlay => state
                .active
                .as_ref()
                .map(|c| (c.surface.clone(), c.viewport.clone())),
        }) else { return };

        {
            // Set viewport destination to the configured size.
            let w = if width  == 0 { 1 } else { width  } as i32;
            let h = if height == 0 { 1 } else { height } as i32;
            viewport.set_destination(w, h);

            let buf = state.get_buffer(qh);
            surface.attach(Some(&buf), 0, 0);
            surface.damage_buffer(0, 0, 1, 1);
            surface.commit();
        }
    }
}

// ── Main entry point ──────────────────────────────────────────────────────────

pub async fn capture_async_wlr(
    tx: mpsc::Sender<InputEvent>,
    mut release_rx: mpsc::Receiver<()>,
    monitors_tx: watch::Sender<Vec<ScreenInfo>>,
    telemetry: Arc<Telemetry>,
) -> Result<()> {
    info!("starting wlr layer-shell capture backend");

    let conn = Connection::connect_to_env().context("connect to WAYLAND_DISPLAY")?;
    let mut event_queue = conn.new_event_queue::<State>();
    let qh = event_queue.handle();

    // Request the registry so the Dispatch impl can bind globals.
    let _registry = conn.display().get_registry(&qh, ());

    let mut state = State {
        compositor: None,
        seat: None,
        layer_shell: None,
        vp_mgr: None,
        ks_mgr: None,
        pc_mgr: None,
        spbuf_mgr: None,
        viewporter: None,
        outputs: Vec::new(),
        wl_outputs: Vec::new(),
        pointer: None,
        keyboard: None,
        rel_ptr_mgr: None,
        relative_pointer: None,
        shared_buffer: None,
        virtual_pointer: None,
        phase: Phase::Idle,
        edge_strips: Vec::new(),
        active: None,
        pending_activation: None,
        deferred_activation: None,
        xkb_mods: Modifiers::default(),
        held_keys: HashSet::new(),
        scroll_acc: (0.0, 0.0),
        tx,
        monitors_tx,
        telemetry,
    };

    // Two roundtrips: one to get globals, one to get initial output state.
    event_queue.roundtrip(&mut state).context("initial registry roundtrip")?;
    event_queue.roundtrip(&mut state).context("output geometry roundtrip")?;

    // Verify required globals.
    if state.compositor.is_none() { anyhow::bail!("wl_compositor not found"); }
    if state.seat.is_none()       { anyhow::bail!("wl_seat not found"); }
    if state.layer_shell.is_none(){ anyhow::bail!("zwlr_layer_shell_v1 not found — compositor does not support it"); }
    if state.viewporter.is_none() { anyhow::bail!("wp_viewporter not found"); }
    if state.spbuf_mgr.is_none()  { anyhow::bail!("wp_single_pixel_buffer_manager_v1 not found"); }

    if state.ks_mgr.is_none() {
        warn!("zwp_keyboard_shortcuts_inhibit_manager_v1 not found — compositor shortcuts will not be suppressed");
    }
    if state.vp_mgr.is_none() {
        warn!("zwlr_virtual_pointer_manager_v1 not found — cursor nudge on release will be skipped");
    }

    // Log discovered screen layout.
    for (i, o) in state.outputs.iter().enumerate() {
        info!("output {i}: {}×{} at ({},{})", o.width, o.height, o.x, o.y);
    }

    // Collect (idx, WlOutput) for strip creation. We need the proxy objects which
    // we bound in the registry handler and stored implicitly in the event queue's
    // object map. Re-binding them from the registry is not straightforward after
    // the initial roundtrip. Instead, we keep a parallel Vec<WlOutput> in state.
    // (See `wl_outputs` field added below — this requires a second pass.)
    //
    // Simpler approach: store WlOutput proxies in a parallel Vec during binding.
    // At this point state.wl_outputs is populated (added to State struct).
    let wl_outputs: Vec<wl_output::WlOutput> = state.wl_outputs.clone();
    let output_pairs: Vec<(usize, wl_output::WlOutput)> = wl_outputs.into_iter().enumerate().collect();

    // Create initial edge strips.
    for (idx, wl_out) in &output_pairs {
        state.create_strips_for_output(*idx, wl_out, &qh);
    }
    // Roundtrip to get configure events for the strips.
    event_queue.roundtrip(&mut state).context("strip configure roundtrip")?;
    info!("wlr capture idle; edge strips armed on {} output(s)", state.outputs.len());

    // Async event loop: interleave Wayland dispatch with Tokio channels.
    let wayland_raw_fd = conn.as_fd().as_raw_fd();
    let async_wl = AsyncFd::new(wayland_raw_fd).context("AsyncFd for Wayland")?;

    loop {
        conn.flush().context("wayland flush")?;

        // biased: release_rx is always checked before Wayland events.
        // This ensures that if route_events rejects a deferred activation (no client
        // at this edge → immediate release_tx), we see it before we process the next
        // Wayland event and accidentally create the overlay for an unconfigured edge.
        tokio::select! {
            biased;

            _ = release_rx.recv() => {
                if state.deferred_activation.is_some() {
                    // route_events said "no client here" before we built the overlay.
                    // Discard quietly — the edge strips are still in place.
                    let edge = state.deferred_activation.take().map(|(e, _, _)| e);
                    debug!("no client at {:?} edge; ignoring activation", edge);
                } else if state.phase == Phase::Active {
                    state.handle_release(&output_pairs, &qh);
                    conn.flush().context("wayland flush post-release")?;
                    event_queue.roundtrip(&mut state).context("strip configure roundtrip on release")?;
                }
            }

            readable = async_wl.readable() => {
                let mut guard = readable.context("wayland fd error")?;
                guard.clear_ready();

                // Phase 2 runs at the TOP — it can only see deferred_activation that was
                // set in a PREVIOUS tick's Phase 1 (which runs at the bottom, after dispatch).
                // biased select already drained any release_rx rejection before we got here.
                // try_recv handles the rare race where readable beat release_rx in the select.
                if let Some((edge, output_idx, cursor)) = state.deferred_activation.take() {
                    if release_rx.try_recv().is_ok() {
                        debug!("no client at {:?} edge (try_recv); discarding activation", edge);
                    } else if state.phase == Phase::Idle {
                        if let Some(wl_out) = state.wl_outputs.get(output_idx).cloned() {
                            state.destroy_edge_strips();
                            state.create_overlay(edge, output_idx, cursor, &wl_out, &qh);
                            conn.flush().context("flush after overlay create")?;
                            event_queue.roundtrip(&mut state).context("overlay configure roundtrip")?;
                        }
                    }
                }

                // Dispatch Wayland events.
                if let Some(read_guard) = event_queue.prepare_read() {
                    read_guard.read().context("wayland read")?;
                }
                event_queue.dispatch_pending(&mut state).context("dispatch")?;

                // Phase 1 runs at the BOTTOM — after dispatch so pending_activation is
                // populated by this tick's Dispatch impls.  Sets deferred_activation for
                // Phase 2 of the NEXT tick; does NOT create the overlay immediately.
                if let Some((edge, output_idx, cursor)) = state.pending_activation.take() {
                    if state.phase == Phase::Idle && state.deferred_activation.is_none() {
                        try_emit(
                            &state.tx,
                            InputEvent::MouseMoveAbs { x: cursor.0, y: cursor.1 },
                            &state.telemetry,
                        );
                        state.deferred_activation = Some((edge, output_idx, cursor));
                    }
                }

                conn.flush().context("wayland flush post-dispatch")?;
            }

            else => break,
        }
    }

    Ok(())
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::Edge;

    #[test]
    fn edge_variants_are_distinct() {
        assert_ne!(Edge::Left, Edge::Right);
        assert_ne!(Edge::Top, Edge::Bottom);
    }
}
