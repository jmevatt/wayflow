// Input capture on Linux/Wayland via the XDG InputCapture portal.
//
// Flow:
//   1. Create an InputCapture session.
//   2. Query zones (monitor regions) and set pointer barriers at every edge.
//   3. Connect to EI and perform the libei handshake as a Receiver.
//   4. Enable capture.
//   5. Loop: wait for Activated signal, forward EI events until release_rx fires.
//   6. On release_rx: call InputCapture::release(), loop back to step 5.
//
// GNOME >= 45 supports InputCapture (portal version 1).

use std::{collections::HashMap, num::NonZeroU32, os::unix::net::UnixStream};

use ashpd::desktop::input_capture::Region;

use anyhow::{Context, Result};
use ashpd::desktop::input_capture::{Barrier, BarrierID, Capabilities, InputCapture};
use futures::StreamExt;
use reis::{
    ei,
    event::{DeviceCapability, EiEvent},
    tokio::{EiConvertEventStream, EiEventStream},
};
use tokio::sync::{mpsc, watch};
use tracing::{debug, info, warn};
use wayflow_proto::{Modifiers, MouseButton, ScreenInfo};

use super::{CaptureBackend, InputEvent};

pub struct LinuxWaylandCapture;

impl CaptureBackend for LinuxWaylandCapture {
    fn start(
        self,
        tx: mpsc::Sender<InputEvent>,
        release_rx: mpsc::Receiver<()>,
        monitors_tx: watch::Sender<Vec<ScreenInfo>>,
    ) -> Result<()> {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("build tokio runtime for capture")?
            .block_on(capture_async(tx, release_rx, monitors_tx))
    }
}

pub fn backend() -> LinuxWaylandCapture {
    LinuxWaylandCapture
}

async fn capture_async(
    tx: mpsc::Sender<InputEvent>,
    mut release_rx: mpsc::Receiver<()>,
    monitors_tx: watch::Sender<Vec<ScreenInfo>>,
) -> Result<()> {
    info!("connecting to XDG InputCapture portal");

    let portal = InputCapture::new().await.context("InputCapture portal")?;

    let capabilities = (Capabilities::Keyboard | Capabilities::Pointer).into();
    let (session, _) = portal
        .create_session(None, capabilities)
        .await
        .context("create_session")?;

    let zones = portal
        .zones(&session)
        .await
        .context("zones")?
        .response()
        .context("zones response")?;

    let zone_set = zones.zone_set();
    let regions: Vec<Region> = zones.regions().to_vec();

    // Publish actual monitor layout to route_events before setting barriers.
    let screen_infos: Vec<ScreenInfo> = regions.iter().map(|r| ScreenInfo {
        name: String::new(),
        x: r.x_offset(),
        y: r.y_offset(),
        width: r.width() as u16,
        height: r.height() as u16,
    }).collect();
    debug!("zones: {:?}", screen_infos.iter().map(|s| format!("{}x{}+{}+{}", s.width, s.height, s.x, s.y)).collect::<Vec<_>>());
    let _ = monitors_tx.send(screen_infos);

    let barriers = build_barriers(&regions);
    debug!("setting {} external barriers across {} zone(s)", barriers.len(), regions.len());

    let barrier_resp = portal
        .set_pointer_barriers(&session, &barriers, zone_set)
        .await
        .context("set_pointer_barriers")?
        .response()
        .context("set_pointer_barriers response")?;

    if !barrier_resp.failed_barriers().is_empty() {
        warn!("some barriers were rejected: {:?}", barrier_resp.failed_barriers());
    }

    let fd = portal.connect_to_eis(&session).await.context("connect_to_eis")?;
    let stream = UnixStream::from(fd);
    let ei_context = ei::Context::new(stream).context("EI context")?;

    let mut raw_events = EiEventStream::new(ei_context.clone()).context("EiEventStream")?;
    let resp = reis::tokio::ei_handshake(
        &mut raw_events,
        "wayflow",
        ei::handshake::ContextType::Receiver,
        &ei_interfaces(),
    )
    .await
    .context("EI handshake")?;

    let mut ei_events = EiConvertEventStream::new(raw_events, resp.serial);

    portal.enable(&session).await.context("enable")?;
    info!("InputCapture enabled; waiting for cursor to hit a barrier");

    let mut activated_stream = portal.receive_activated().await.context("receive_activated")?;
    let mut deactivated_stream = portal.receive_deactivated().await.context("receive_deactivated")?;

    let mut active: Option<ActivationState> = None;
    let mut modifiers = Modifiers::default();

    loop {
        tokio::select! {
            Some(evt) = activated_stream.next() => {
                let pos = evt.cursor_position().unwrap_or((0.0, 0.0));
                let activation_id = evt.activation_id();
                debug!("capture activated id={activation_id:?} pos={pos:?}");

                let _ = tx.send(InputEvent::MouseMoveAbs {
                    x: pos.0 as f64,
                    y: pos.1 as f64,
                }).await;

                let pos_f64 = (pos.0 as f64, pos.1 as f64);
                active = Some(ActivationState {
                    activation_id,
                    activation_pos: pos_f64,
                    cursor_pos: pos_f64,
                });
            }

            Some(_) = deactivated_stream.next() => {
                debug!("capture deactivated by compositor");
                active = None;
            }

            Some(ei_result) = ei_events.next() => {
                match ei_result {
                    Err(e) => { warn!("EI stream error: {e:?}"); break; }
                    Ok(event) => {
                        handle_ei_event(event, &tx, &ei_context, &mut active, &mut modifiers).await;
                    }
                }
            }

            Some(()) = release_rx.recv() => {
                if let Some(state) = active.take() {
                    debug!("releasing capture activation_id={:?}", state.activation_id);
                    // Return cursor to where it departed the server (the barrier hit
                    // point), nudged 5px inside so it doesn't immediately re-trigger.
                    // Using activation_pos rather than the accumulated cursor_pos
                    // prevents over-travel on the client from placing the cursor at
                    // the wrong edge of the server on return.
                    let release_pos = nudge_inside(state.activation_pos, &regions);
                    portal
                        .release(&session, state.activation_id, Some(release_pos))
                        .await
                        .ok();
                }
            }

            else => break,
        }
    }

    Ok(())
}

struct ActivationState {
    activation_id: Option<u32>,
    /// Position where the cursor originally hit the barrier. Used for the
    /// portal release call so the cursor returns to the departure point
    /// rather than an over-traveled accumulated position.
    activation_pos: (f64, f64),
    /// Running absolute position, updated by EI PointerMotion deltas.
    cursor_pos: (f64, f64),
}

async fn handle_ei_event(
    event: EiEvent,
    tx: &mpsc::Sender<InputEvent>,
    context: &ei::Context,
    active: &mut Option<ActivationState>,
    modifiers: &mut Modifiers,
) {
    match event {
        EiEvent::SeatAdded(evt) => {
            evt.seat.bind_capabilities(&[
                DeviceCapability::Pointer,
                DeviceCapability::PointerAbsolute,
                DeviceCapability::Keyboard,
                DeviceCapability::Scroll,
                DeviceCapability::Button,
            ]);
            context.flush().ok();
            debug!("EI seat bound");
        }

        EiEvent::PointerMotion(evt) if active.is_some() => {
            let state = active.as_mut().unwrap();
            state.cursor_pos.0 += evt.dx as f64;
            state.cursor_pos.1 += evt.dy as f64;
            let _ = tx.send(InputEvent::MouseMoveAbs {
                x: state.cursor_pos.0,
                y: state.cursor_pos.1,
            }).await;
        }

        EiEvent::Button(evt) if active.is_some() => {
            use reis::ei::button::ButtonState;
            let button = evdev_to_mouse_button(evt.button);
            let pressed = evt.state == ButtonState::Press;
            let _ = tx.send(InputEvent::MouseButton { button, pressed }).await;
        }

        EiEvent::ScrollDelta(evt) if active.is_some() => {
            let _ = tx.send(InputEvent::Scroll {
                dx: evt.dx as f64,
                dy: evt.dy as f64,
            }).await;
        }

        EiEvent::KeyboardKey(evt) if active.is_some() => {
            use reis::ei::keyboard::KeyState;
            if let Some(hid) = wayflow_core::keymap::evdev::evdev_to_hid(evt.key) {
                let pressed = evt.state == KeyState::Press;
                let _ = tx.send(InputEvent::Key { keycode: hid, pressed, modifiers: *modifiers }).await;
            } else {
                debug!("evdev key {} has no HID mapping, skipping", evt.key);
            }
        }

        EiEvent::KeyboardModifiers(evt) => {
            *modifiers = xkb_mods_to_proto(evt.depressed);
        }

        _ => {}
    }
}

fn ei_interfaces() -> HashMap<&'static str, u32> {
    let mut m = HashMap::new();
    m.insert("ei_connection", 1);
    m.insert("ei_callback", 1);
    m.insert("ei_pingpong", 1);
    m.insert("ei_seat", 1);
    m.insert("ei_device", 2);
    m.insert("ei_pointer", 1);
    m.insert("ei_pointer_absolute", 1);
    m.insert("ei_scroll", 1);
    m.insert("ei_button", 1);
    m.insert("ei_keyboard", 1);
    m
}

fn evdev_to_mouse_button(code: u32) -> MouseButton {
    match code {
        0x110 => MouseButton::Left,
        0x111 => MouseButton::Right,
        0x112 => MouseButton::Middle,
        0x113 => MouseButton::Back,
        0x114 => MouseButton::Forward,
        n     => MouseButton::Other((n & 0xff) as u8),
    }
}

fn xkb_mods_to_proto(depressed: u32) -> Modifiers {
    Modifiers {
        shift: (depressed & 0x01) != 0,
        ctrl:  (depressed & 0x04) != 0,
        alt:   (depressed & 0x08) != 0,
        meta:  (depressed & 0x40) != 0,
    }
}

fn nudge_inside(pos: (f64, f64), regions: &[Region]) -> (f64, f64) {
    let (x, y) = pos;
    let margin = 5.0_f64;
    for r in regions {
        let rx  = r.x_offset() as f64;
        let ry  = r.y_offset() as f64;
        let rx2 = rx + r.width()  as f64 - 1.0;
        let ry2 = ry + r.height() as f64 - 1.0;
        // Check if we're at a horizontal barrier (cursor y within region, x at edge)
        if y >= ry && y <= ry2 {
            if x <= rx  { return (rx  + margin, y); }
            if x >= rx2 { return (rx2 - margin, y); }
        }
        // Check if we're at a vertical barrier (cursor x within region, y at edge)
        if x >= rx && x <= rx2 {
            if y <= ry  { return (x, ry  + margin); }
            if y >= ry2 { return (x, ry2 - margin); }
        }
    }
    pos
}

/// Returns true if `region` has another region directly adjacent on its right edge
/// (i.e., the two regions share an internal seam and no external barrier should be placed there).
fn has_right_neighbor(regions: &[Region], r: &Region) -> bool {
    let x2 = r.x_offset() + r.width() as i32;
    let ry = r.y_offset();
    let ry2 = ry + r.height() as i32;
    regions.iter().any(|o| {
        o.x_offset() == x2
            && o.y_offset() < ry2
            && o.y_offset() + o.height() as i32 > ry
    })
}

fn has_left_neighbor(regions: &[Region], r: &Region) -> bool {
    let rx = r.x_offset();
    let ry = r.y_offset();
    let ry2 = ry + r.height() as i32;
    regions.iter().any(|o| {
        o.x_offset() + o.width() as i32 == rx
            && o.y_offset() < ry2
            && o.y_offset() + o.height() as i32 > ry
    })
}

fn has_bottom_neighbor(regions: &[Region], r: &Region) -> bool {
    let y2 = r.y_offset() + r.height() as i32;
    let rx = r.x_offset();
    let rx2 = rx + r.width() as i32;
    regions.iter().any(|o| {
        o.y_offset() == y2
            && o.x_offset() < rx2
            && o.x_offset() + o.width() as i32 > rx
    })
}

fn has_top_neighbor(regions: &[Region], r: &Region) -> bool {
    let ry = r.y_offset();
    let rx = r.x_offset();
    let rx2 = rx + r.width() as i32;
    regions.iter().any(|o| {
        o.y_offset() + o.height() as i32 == ry
            && o.x_offset() < rx2
            && o.x_offset() + o.width() as i32 > rx
    })
}

fn build_barriers(regions: &[Region]) -> Vec<Barrier> {
    let mut barriers = Vec::with_capacity(regions.len() * 4);
    let mut id: u32 = 1;
    let nz = |n: u32| -> BarrierID { NonZeroU32::new(n).unwrap() };
    for region in regions {
        let x  = region.x_offset();
        let y  = region.y_offset();
        let x2 = x + region.width()  as i32 - 1;
        let y2 = y + region.height() as i32 - 1;
        // Only place a barrier on edges that are the outer boundary of the desktop.
        // Skip edges where an adjacent monitor forms an internal seam.
        if !has_right_neighbor(regions, region)  { barriers.push(Barrier::new(nz(id), (x2, y,  x2, y2))); id += 1; }
        if !has_left_neighbor(regions, region)   { barriers.push(Barrier::new(nz(id), (x,  y,  x,  y2))); id += 1; }
        if !has_bottom_neighbor(regions, region) { barriers.push(Barrier::new(nz(id), (x,  y2, x2, y2))); id += 1; }
        if !has_top_neighbor(regions, region)    { barriers.push(Barrier::new(nz(id), (x,  y,  x2, y)));  id += 1; }
    }
    barriers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_fn_returns_capture() {
        let _b = backend();
    }

    #[test]
    fn evdev_to_mouse_button_known() {
        assert!(matches!(evdev_to_mouse_button(0x110), MouseButton::Left));
        assert!(matches!(evdev_to_mouse_button(0x111), MouseButton::Right));
        assert!(matches!(evdev_to_mouse_button(0x112), MouseButton::Middle));
        assert!(matches!(evdev_to_mouse_button(0x113), MouseButton::Back));
        assert!(matches!(evdev_to_mouse_button(0x114), MouseButton::Forward));
    }

    #[test]
    fn evdev_to_mouse_button_unknown() {
        assert!(matches!(evdev_to_mouse_button(0x115), MouseButton::Other(_)));
    }

    #[test]
    fn xkb_mods_shift_only() {
        let m = xkb_mods_to_proto(0x01);
        assert!(m.shift && !m.ctrl && !m.alt && !m.meta);
    }

    #[test]
    fn xkb_mods_all() {
        let m = xkb_mods_to_proto(0x01 | 0x04 | 0x08 | 0x40);
        assert!(m.shift && m.ctrl && m.alt && m.meta);
    }

    #[test]
    fn xkb_mods_none() {
        let m = xkb_mods_to_proto(0x00);
        assert!(!m.shift && !m.ctrl && !m.alt && !m.meta);
    }

    #[test]
    fn build_barriers_empty_regions() {
        let barriers = build_barriers(&[]);
        assert!(barriers.is_empty());
    }
}
