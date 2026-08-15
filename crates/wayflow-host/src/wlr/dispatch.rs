//! Wayland event plumbing.

use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_output, wl_pointer, wl_registry, wl_seat, wl_surface,
};
use wayland_client::{delegate_noop, Connection, Dispatch, QueueHandle, WEnum};
use wayland_protocols::wp::single_pixel_buffer::v1::client::wp_single_pixel_buffer_manager_v1 as spbuf;
use wayland_protocols::wp::viewporter::client::{wp_viewport, wp_viewporter};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1 as ls, zwlr_layer_surface_v1 as lsv,
};

use super::state::{EdgeStrip, OutputInfo, Phase, State, SurfaceRole};

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        (): &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        else {
            return;
        };
        match interface.as_str() {
            "wl_compositor" => {
                state.compositor = Some(registry.bind(name, version.min(6), qh, ()));
            }
            "zwlr_layer_shell_v1" => {
                state.layer_shell = Some(registry.bind(name, version.min(5), qh, ()));
            }
            "wp_viewporter" => {
                state.viewporter = Some(registry.bind(name, 1, qh, ()));
            }
            "wp_single_pixel_buffer_manager_v1" => {
                state.spbuf_mgr = Some(registry.bind(name, 1, qh, ()));
            }
            "wl_seat" => {
                state.seat = Some(registry.bind(name, version.min(9), qh, ()));
            }
            "zwp_pointer_constraints_v1" => {
                state.pc_mgr = Some(registry.bind(name, 1, qh, ()));
            }
            "zwp_relative_pointer_manager_v1" => {
                state.rel_ptr_mgr = Some(registry.bind(name, 1, qh, ()));
            }
            "zwp_keyboard_shortcuts_inhibit_manager_v1" => {
                state.ks_mgr = Some(registry.bind(name, 1, qh, ()));
            }
            "zwlr_virtual_pointer_manager_v1" => {
                state.vp_mgr = Some(registry.bind(name, version.min(2), qh, ()));
            }
            "wl_output" => {
                let idx = state.outputs.len();
                let output = registry.bind(name, version.min(4), qh, idx);
                state.outputs.push((output, OutputInfo::default()));
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_output::WlOutput, usize> for State {
    fn event(
        state: &mut Self,
        _output: &wl_output::WlOutput,
        event: wl_output::Event,
        &idx: &usize,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let Some((_, info)) = state.outputs.get_mut(idx) else {
            return;
        };
        match event {
            wl_output::Event::Geometry { x, y, .. } => {
                info.x = x;
                info.y = y;
            }
            // Only the current mode carries the resolution we place against; the
            // compositor also advertises every other mode the monitor supports.
            wl_output::Event::Mode {
                flags,
                width,
                height,
                ..
            } => {
                if matches!(flags, WEnum::Value(f) if f.contains(wl_output::Mode::Current)) {
                    info.width = width;
                    info.height = height;
                }
            }
            wl_output::Event::Done => info.done = true,
            _ => {}
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        (): &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
        {
            if caps.contains(wl_seat::Capability::Pointer) && state.pointer.is_none() {
                state.pointer = Some(seat.get_pointer(qh, ()));
            }
            if caps.contains(wl_seat::Capability::Keyboard) && state.keyboard.is_none() {
                state.keyboard = Some(seat.get_keyboard(qh, ()));
            }
        }
    }
}

impl Dispatch<lsv::ZwlrLayerSurfaceV1, SurfaceRole> for State {
    fn event(
        state: &mut Self,
        layer_surface: &lsv::ZwlrLayerSurfaceV1,
        event: lsv::Event,
        role: &SurfaceRole,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let lsv::Event::Configure {
            serial,
            width,
            height,
        } = event
        else {
            return;
        };
        // Acknowledging is mandatory; attaching a buffer before this point is a protocol
        // error that kills the connection rather than degrading.
        layer_surface.ack_configure(serial);

        let blank = state.blank_buffer(qh);
        // The compositor reports the size it actually granted, which is authoritative:
        // our requests left an axis as zero precisely so it would fill in the real value.
        let w = i32::try_from(width).unwrap_or(1).max(1);
        let h = i32::try_from(height).unwrap_or(1).max(1);

        match *role {
            SurfaceRole::Strip => {
                let Some(strip) = &mut state.strip else {
                    return;
                };
                if strip.configured {
                    return;
                }
                strip.viewport.set_destination(w, h);
                strip.surface.attach(Some(&blank), 0, 0);
                strip.surface.commit();
                strip.configured = true;
                println!("edge strip mapped: {w}x{h}");
            }
            SurfaceRole::Overlay => {
                // The overlay must be mapped, not merely created. An unmapped layer
                // surface receives no keyboard focus and no pointer lock, so skipping the
                // buffer here silently produced a capture that forwarded nothing. The
                // buffer is a single fully transparent pixel stretched over the screen,
                // so it takes focus while remaining invisible.
                let Some(active) = &mut state.active else {
                    return;
                };
                if active.configured {
                    return;
                }
                active.viewport.set_destination(w, h);
                active.surface.attach(Some(&blank), 0, 0);
                active.surface.commit();
                active.configured = true;
                eprintln!("wayflow-host: overlay mapped {w}x{h}, input held");
            }
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for State {
    fn event(
        state: &mut Self,
        pointer: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        (): &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            // Entering the strip means the compositor's own cursor reached the desktop
            // boundary. That is the signal, and it is the compositor's truth rather than
            // an accumulation of raw deltas that pointer acceleration would have skewed.
            wl_pointer::Event::Enter {
                serial,
                surface,
                surface_y,
                ..
            } if state.is_strip(&surface) && state.phase == Phase::Idle => {
                // Where along the edge the pointer arrived, so the client can place its
                // cursor at the matching height and the crossing looks continuous.
                let height = state.strip_height().max(1.0);
                let ratio = (surface_y / height).clamp(0.0, 1.0);
                state.begin_capture(ratio, qh);
                // Hide only once capture actually took. `begin_capture` declines when no
                // client is reachable, and hiding regardless would leave an invisible
                // cursor on a machine that never handed control anywhere.
                if state.phase == Phase::Active {
                    // Hiding on the strip's own serial rather than waiting for the
                    // overlay's enter: that arrives a frame or two later, and the arrow
                    // lingering at the boundary reads as a stutter, not a handoff.
                    pointer.set_cursor(serial, None, 0, 0);
                }
            }
            // The overlay covers the screen, so the pointer enters it immediately after
            // capture begins. A null cursor surface is how Wayland says "draw nothing";
            // the pointer is still there and still locked, it simply has no image.
            wl_pointer::Event::Enter {
                serial, surface, ..
            } if state.is_overlay(&surface) => {
                pointer.set_cursor(serial, None, 0, 0);
            }
            ref other => state.forward_pointer(other),
        }
    }
}

impl State {
    pub(crate) fn is_overlay(&self, surface: &wl_surface::WlSurface) -> bool {
        self.active.as_ref().is_some_and(|a| &a.surface == surface)
    }

    pub(crate) fn is_strip(&self, surface: &wl_surface::WlSurface) -> bool {
        self.strip.as_ref().is_some_and(|s| &s.surface == surface)
    }

    /// Put the one-pixel strip on the boundary output for the configured edge.
    pub fn place_edge_strip(&mut self, qh: &QueueHandle<Self>) {
        let Some(idx) = self.boundary_output() else {
            return;
        };
        let output = self.outputs[idx].0.clone();
        let geometry = self.outputs[idx].1.clone();
        let edge = self.config.edge;

        let compositor = self.compositor.clone().expect("checked in check_globals");
        let shell = self.layer_shell.clone().expect("checked in check_globals");
        let viewporter = self.viewporter.clone().expect("checked in check_globals");

        let surface = compositor.create_surface(qh, ());
        let layer_surface = shell.get_layer_surface(
            &surface,
            Some(&output),
            ls::Layer::Overlay,
            "wayflow-edge".to_owned(),
            qh,
            SurfaceRole::Strip,
        );
        layer_surface.set_anchor(edge.anchors());
        let (w, h) = edge.strip_size();
        layer_surface.set_size(w, h);
        // Zero keeps the strip from displacing tiled windows; it overlaps them instead.
        layer_surface.set_exclusive_zone(0);
        layer_surface.set_keyboard_interactivity(lsv::KeyboardInteractivity::None);

        let viewport = viewporter.get_viewport(&surface, qh, ());
        // Commit with no buffer attached. The compositor answers with a configure naming
        // the size it granted, and the buffer goes on only after that is acknowledged.
        surface.commit();

        println!(
            "edge strip: {edge:?} on output {idx} ({}x{} at {},{})",
            geometry.width, geometry.height, geometry.x, geometry.y
        );
        self.strip = Some(EdgeStrip {
            surface,
            layer_surface,
            viewport,
            configured: false,
        });
    }
}

delegate_noop!(State: ignore wl_compositor::WlCompositor);
delegate_noop!(State: ignore wl_surface::WlSurface);
delegate_noop!(State: ignore ls::ZwlrLayerShellV1);
delegate_noop!(State: ignore wp_viewporter::WpViewporter);
delegate_noop!(State: ignore wp_viewport::WpViewport);
delegate_noop!(State: ignore spbuf::WpSinglePixelBufferManagerV1);
delegate_noop!(State: ignore wl_buffer::WlBuffer);
