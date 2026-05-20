// Input capture on Linux/Wayland via the XDG InputCapture portal.
//
// Flow:
//   1. Create an InputCapture session.
//   2. Query zones (monitor regions) and set pointer barriers at every edge.
//   3. Connect to EI and perform the libei handshake as a Receiver.
//   4. Enable capture.
//   5. Loop: wait for Activated signal, forward EI events until release_rx fires.
//   6. On release_rx: call InputCapture::release(), loop back to step 5.
//   7. On ZonesChanged: close the stale session and rebuild from step 1.
//
// GNOME >= 45 supports InputCapture (portal version 1).

use std::sync::atomic::Ordering;
use std::{collections::HashMap, num::NonZeroU32, os::unix::net::UnixStream, sync::Arc};

use ashpd::desktop::input_capture::Region;

use anyhow::{Context, Result};
use ashpd::desktop::input_capture::{
    Activated, ActivatedBarrier, Barrier, BarrierID, Capabilities, InputCapture,
};
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
use crate::telemetry::Telemetry;

/// Non-blocking emit for idempotent / floody events (cursor motion, scroll
/// ticks). Drops on a full channel rather than back-pressuring libei, since
/// these events tolerate loss: a missed cursor sample is corrected by the
/// next sample, and a missed scroll tick is a one-frame stutter.
///
/// Do NOT use this for state-changing events (Button, Key) -- a dropped key
/// release leaves the client with a phantom held key (commonly: stuck shift
/// after fast typing). Those go through `emit_blocking` instead.
pub(super) fn try_emit(tx: &mpsc::Sender<InputEvent>, event: InputEvent, telemetry: &Telemetry) {
    use mpsc::error::TrySendError;
    match tx.try_send(event) {
        Ok(()) => {}
        Err(TrySendError::Full(dropped)) => {
            telemetry
                .input_events_dropped_full
                .fetch_add(1, Ordering::Relaxed);
            warn!("input pipeline full (256 cap reached); dropping {dropped:?}");
        }
        Err(TrySendError::Closed(_)) => {
            telemetry
                .input_events_dropped_closed
                .fetch_add(1, Ordering::Relaxed);
            // route_events task has exited -- server is shutting down. Ignore.
        }
    }
}

/// Blocking emit for state-changing events (Button, Key, synthesized
/// modifier press/release). Mirrors `server::send_s2c_timed`: we accept
/// back-pressure rather than drop, because dropping a key release would
/// leave the client holding a phantom key (typed as `Shift+a` -> shift-up
/// dropped -> mac client thinks shift is still held -> every subsequent
/// keystroke is uppercase). Logs a slow-emit warning + telemetry counter
/// so back-pressure stays visible without sacrificing correctness.
pub(super) async fn emit_blocking(tx: &mpsc::Sender<InputEvent>, event: InputEvent, telemetry: &Telemetry) {
    const SLOW_THRESHOLD: tokio::time::Duration = tokio::time::Duration::from_millis(50);
    let start = tokio::time::Instant::now();
    if tx.send(event).await.is_err() {
        // Receiver gone -- server shutting down. Match try_emit's silent close handling.
        telemetry
            .input_events_dropped_closed
            .fetch_add(1, Ordering::Relaxed);
        return;
    }
    let elapsed = start.elapsed();
    if elapsed > SLOW_THRESHOLD {
        telemetry.capture_slow_emits.fetch_add(1, Ordering::Relaxed);
        warn!(
            "capture emit took {:?} -- route_events is back-pressured",
            elapsed
        );
    }
}

/// HID usage codes for the standard modifier keys.
/// EI's `KeyboardModifiers` event reports a state bitmap (shift/ctrl/alt/meta)
/// without telling us which physical key is depressed; the synth-at-activation
/// path always picks the left variant. The macOS Window Server tracks left
/// and right modifier flags independently, so a synthesised L press paired
/// with a real R release leaves the L flag stuck. `modifier_diff_actions`
/// reconciles this on every modifier-bitmap change: if a bit transitions
/// 1 -> 0 while the L variant is still in `held_keys`, it emits a synth
/// release for whichever variant is held.
const HID_LEFT_CTRL: u32 = 0xE0;
const HID_LEFT_SHIFT: u32 = 0xE1;
const HID_LEFT_ALT: u32 = 0xE2;
const HID_LEFT_META: u32 = 0xE3;
const HID_RIGHT_CTRL: u32 = 0xE4;
const HID_RIGHT_SHIFT: u32 = 0xE5;
const HID_RIGHT_ALT: u32 = 0xE6;
const HID_RIGHT_META: u32 = 0xE7;

fn modifier_hid_codes(mods: Modifiers) -> impl Iterator<Item = u32> {
    [
        mods.shift.then_some(HID_LEFT_SHIFT),
        mods.ctrl.then_some(HID_LEFT_CTRL),
        mods.alt.then_some(HID_LEFT_ALT),
        mods.meta.then_some(HID_LEFT_META),
    ]
    .into_iter()
    .flatten()
}

/// Reconcile the modifier-bitmap diff against `held_keys` and return the
/// synth Key events the caller should emit (in order).
///
/// Handles two asymmetries that leave a stuck modifier on the client:
///
/// 1. *Right-modifier held at activation.* The activation-time synth always
///    presses the L variant, but EI's eventual KeyboardKey release carries
///    the R keycode. On 1 -> 0 transition we emit a release for whichever
///    variant is still in `held_keys`.
///
/// 2. *Bitmap-only compositors.* Some setups report modifier presses only
///    via KeyboardModifiers, never as a KeyboardKey event. On 0 -> 1
///    transition with no L or R variant tracked, we synth a press for L.
fn modifier_diff_actions(
    old_mods: Modifiers,
    new_mods: Modifiers,
    held_keys: &std::collections::HashSet<u32>,
    active: bool,
) -> Vec<(u32, bool)> {
    if !active {
        return Vec::new();
    }
    let mut actions = Vec::new();
    for (old_bit, new_bit, left_hid, right_hid) in [
        (old_mods.shift, new_mods.shift, HID_LEFT_SHIFT, HID_RIGHT_SHIFT),
        (old_mods.ctrl,  new_mods.ctrl,  HID_LEFT_CTRL,  HID_RIGHT_CTRL),
        (old_mods.alt,   new_mods.alt,   HID_LEFT_ALT,   HID_RIGHT_ALT),
        (old_mods.meta,  new_mods.meta,  HID_LEFT_META,  HID_RIGHT_META),
    ] {
        if old_bit && !new_bit {
            // 1 -> 0: release whichever synth-press is still tracked.
            if held_keys.contains(&left_hid) {
                actions.push((left_hid, false));
            } else if held_keys.contains(&right_hid) {
                actions.push((right_hid, false));
            }
        } else if !old_bit && new_bit && !held_keys.contains(&left_hid) && !held_keys.contains(&right_hid) {
            // 0 -> 1 with no Key event tracking either variant: synth L.
            actions.push((left_hid, true));
        }
    }
    actions
}

pub struct LinuxWaylandCapture;

impl CaptureBackend for LinuxWaylandCapture {
    fn start(
        self,
        tx: mpsc::Sender<InputEvent>,
        release_rx: mpsc::Receiver<()>,
        monitors_tx: watch::Sender<Vec<ScreenInfo>>,
        telemetry: Arc<Telemetry>,
    ) -> Result<()> {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("build tokio runtime for capture")?
            .block_on(capture_async(tx, release_rx, monitors_tx, telemetry))
    }
}

pub fn backend() -> LinuxWaylandCapture {
    LinuxWaylandCapture
}

async fn capture_async(
    tx: mpsc::Sender<InputEvent>,
    mut release_rx: mpsc::Receiver<()>,
    monitors_tx: watch::Sender<Vec<ScreenInfo>>,
    telemetry: Arc<Telemetry>,
) -> Result<()> {
    info!("connecting to XDG InputCapture portal");

    let portal = match InputCapture::new().await {
        Ok(p) => p,
        Err(e) => {
            let msg = e.to_string();
            // D-Bus / portal not available: fall back to wlroots layer-shell backend.
            // xdg-desktop-portal-wlr does not implement InputCapture.
            if msg.contains("not found")
                || msg.contains("ServiceUnknown")
                || msg.contains("NameHasNoOwner")
            {
                info!("InputCapture portal unavailable ({msg}); falling back to wlr layer-shell backend");
                return super::linux_wayland_wlr::capture_async_wlr(
                    tx,
                    release_rx,
                    monitors_tx,
                    telemetry,
                )
                .await;
            }
            return Err(anyhow::Error::from(e).context("InputCapture portal"));
        }
    };

    let mut zones_changed_stream = portal
        .receive_zones_changed()
        .await
        .context("receive_zones_changed")?;

    while let CaptureSessionOutcome::ZonesChanged = capture_session(
        &portal,
        &mut zones_changed_stream,
        &tx,
        &mut release_rx,
        &monitors_tx,
        &telemetry,
    )
    .await?
    {
        info!("display zones changed; rebuilding InputCapture session");
    }

    Ok(())
}

enum CaptureSessionOutcome {
    ZonesChanged,
    Stopped,
}

async fn capture_session(
    portal: &InputCapture<'_>,
    zones_changed_stream: &mut (impl futures::Stream<Item = ashpd::desktop::input_capture::ZonesChanged>
              + Unpin),
    tx: &mpsc::Sender<InputEvent>,
    release_rx: &mut mpsc::Receiver<()>,
    monitors_tx: &watch::Sender<Vec<ScreenInfo>>,
    telemetry: &Telemetry,
) -> Result<CaptureSessionOutcome> {
    let capabilities = Capabilities::Keyboard | Capabilities::Pointer;
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
    let screen_infos: Vec<ScreenInfo> = regions
        .iter()
        .map(|r| ScreenInfo {
            name: String::new(),
            x: r.x_offset(),
            y: r.y_offset(),
            width: r.width() as u16,
            height: r.height() as u16,
        })
        .collect();
    debug!(
        "zones: {:?}",
        screen_infos
            .iter()
            .map(|s| format!("{}x{}+{}+{}", s.width, s.height, s.x, s.y))
            .collect::<Vec<_>>()
    );
    let _ = monitors_tx.send(screen_infos);

    let (barriers, barrier_dirs) = build_barriers(&regions);
    debug!(
        "setting {} external barriers across {} zone(s)",
        barriers.len(),
        regions.len()
    );

    let barrier_resp = portal
        .set_pointer_barriers(&session, &barriers, zone_set)
        .await
        .context("set_pointer_barriers")?
        .response()
        .context("set_pointer_barriers response")?;

    if !barrier_resp.failed_barriers().is_empty() {
        warn!(
            "some barriers were rejected: {:?}",
            barrier_resp.failed_barriers()
        );
    }

    let fd = portal
        .connect_to_eis(&session)
        .await
        .context("connect_to_eis")?;
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

    let mut activated_stream = portal
        .receive_activated()
        .await
        .context("receive_activated")?;
    let mut deactivated_stream = portal
        .receive_deactivated()
        .await
        .context("receive_deactivated")?;

    let mut active: Option<ActivationState> = None;
    let mut modifiers = Modifiers::default();
    let mut scroll_remainder = (0.0_f64, 0.0_f64);
    // Keys currently held according to EI events we've forwarded to route_events,
    // *plus* any modifiers we've synthesized at activation. Used to flush
    // synthetic release events on deactivation so the client never gets stuck
    // modifiers when the compositor unilaterally deactivates capture or when
    // a select! race processes Deactivated before a trailing key-release event.
    let mut held_keys: std::collections::HashSet<u32> = std::collections::HashSet::new();

    loop {
        tokio::select! {
            Some(evt) = activated_stream.next() => {
                let pos = evt.cursor_position().unwrap_or((0.0, 0.0));
                let activation_id = evt.activation_id();
                let nudge_dir = nudge_dir_for_activation(&evt, &barrier_dirs);
                debug!("capture activated id={activation_id:?} pos={pos:?} nudge_dir={nudge_dir:?}");

                try_emit(tx, InputEvent::MouseMoveAbs {
                    x: pos.0 as f64,
                    y: pos.1 as f64,
                }, telemetry);

                // Synthesize key-press events for any modifiers currently held.
                // EI reports modifier state but does NOT replay individual key
                // presses for already-held keys at activation -- without this
                // synthesis the client would never see the press, and
                // shift/ctrl/alt/meta combos formed by holding a modifier
                // before crossing the screen edge would silently drop the modifier.
                for hid in modifier_hid_codes(modifiers) {
                    if held_keys.insert(hid) {
                        debug!("synth modifier press at activation: {hid:#x}");
                        emit_blocking(tx, InputEvent::Key {
                            keycode: hid,
                            pressed: true,
                            modifiers,
                        }, telemetry).await;
                    }
                }

                let pos_f64 = (pos.0 as f64, pos.1 as f64);
                active = Some(ActivationState {
                    activation_id,
                    nudge_dir,
                    activation_pos: pos_f64,
                    cursor_pos: pos_f64,
                });
            }

            Some(_) = deactivated_stream.next() => {
                debug!("capture deactivated by compositor");
                // Flush synthetic releases for every key we believe is held.
                // Catches the select! race where a trailing key-release EI
                // event would otherwise be dropped because `active` flipped
                // to None first; also catches the unilateral compositor
                // deactivation case where `route_events` doesn't get a
                // mouse-cross to trigger its own held_keys flush.
                for hid in held_keys.drain() {
                    debug!("synth release at deactivation: {hid:#x}");
                    emit_blocking(tx, InputEvent::Key {
                        keycode: hid,
                        pressed: false,
                        modifiers: Modifiers::default(),
                    }, telemetry).await;
                }
                active = None;
            }

            Some(ei_result) = ei_events.next() => {
                match ei_result {
                    Err(e) => { warn!("EI stream error: {e:?}"); break; }
                    Ok(event) => {
                        handle_ei_event(event, tx, &ei_context, &mut active, &mut modifiers, &mut held_keys, &mut scroll_remainder, telemetry).await;
                    }
                }
            }

            Some(()) = release_rx.recv() => {
                if let Some(state) = active.take() {
                    // Nudge 5px away from the barrier. Prefer the barrier_id-based
                    // direction (exact axis) over position inference (nudge_inside),
                    // which can leave the cursor at a second barrier when cursor_position
                    // returns (0,0) due to the GNOME portal not filling the field.
                    let release_pos = match state.nudge_dir {
                        Some(dir) => nudge_by_dir(state.activation_pos, dir),
                        None      => nudge_inside(state.activation_pos, &regions),
                    };
                    debug!("releasing id={:?} activation={:?} dir={:?} -> release={:?}",
                        state.activation_id, state.activation_pos, state.nudge_dir, release_pos);
                    portal
                        .release(&session, state.activation_id, Some(release_pos))
                        .await
                        .ok();
                }
            }

            Some(evt) = zones_changed_stream.next() => {
                info!("InputCapture zones changed: zone_set={:?}", evt.zone_set());
                if let Some(state) = active.take() {
                    let release_pos = match state.nudge_dir {
                        Some(dir) => nudge_by_dir(state.activation_pos, dir),
                        None => nudge_inside(state.activation_pos, &regions),
                    };
                    portal
                        .release(&session, state.activation_id, Some(release_pos))
                        .await
                        .ok();
                }
                emit_blocking(tx, InputEvent::CaptureReset, telemetry).await;
                session.close().await.ok();
                return Ok(CaptureSessionOutcome::ZonesChanged);
            }

            else => return Ok(CaptureSessionOutcome::Stopped),
        }
    }

    Ok(CaptureSessionOutcome::Stopped)
}

/// Which side of the desktop the triggered barrier is on.
/// Used to nudge the cursor perpendicular to the barrier on release.
#[derive(Clone, Copy, Debug)]
enum NudgeDir {
    Left,
    Right,
    Top,
    Bottom,
}

struct ActivationState {
    activation_id: Option<u32>,
    /// Which barrier edge fired, if the compositor reported it.
    nudge_dir: Option<NudgeDir>,
    /// Position where the cursor originally hit the barrier. Used for the
    /// portal release call so the cursor returns to the departure point
    /// rather than an over-traveled accumulated position.
    activation_pos: (f64, f64),
    /// Running absolute position, updated by EI PointerMotion deltas.
    cursor_pos: (f64, f64),
}

#[allow(clippy::too_many_arguments)]
async fn handle_ei_event(
    event: EiEvent,
    tx: &mpsc::Sender<InputEvent>,
    context: &ei::Context,
    active: &mut Option<ActivationState>,
    modifiers: &mut Modifiers,
    held_keys: &mut std::collections::HashSet<u32>,
    scroll_remainder: &mut (f64, f64),
    telemetry: &Telemetry,
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
            try_emit(
                tx,
                InputEvent::MouseMoveAbs {
                    x: state.cursor_pos.0,
                    y: state.cursor_pos.1,
                },
                telemetry,
            );
        }

        EiEvent::Button(evt) if active.is_some() => {
            use reis::ei::button::ButtonState;
            let button = evdev_to_mouse_button(evt.button);
            let pressed = evt.state == ButtonState::Press;
            emit_blocking(tx, InputEvent::MouseButton { button, pressed }, telemetry).await;
        }

        EiEvent::ScrollDelta(evt) if active.is_some() => {
            // EI uses opposite scroll direction from Wayland/rdev conventions -- negate.
            // 10 pixels = 1 scroll click (mutter/gtk historical constant from deskflow).
            // Accumulate sub-pixel remainder so touchpad high-res events don't get lost.
            const PIXELS_PER_CLICK: f64 = 10.0;
            scroll_remainder.0 += -(evt.dx as f64) / PIXELS_PER_CLICK;
            scroll_remainder.1 += -(evt.dy as f64) / PIXELS_PER_CLICK;
            let ix = scroll_remainder.0.trunc();
            let iy = scroll_remainder.1.trunc();
            scroll_remainder.0 -= ix;
            scroll_remainder.1 -= iy;
            if ix != 0.0 || iy != 0.0 {
                try_emit(tx, InputEvent::Scroll { dx: ix, dy: iy }, telemetry);
            }
        }

        EiEvent::ScrollDiscrete(evt) if active.is_some() => {
            // discrete units are in 120ths of a scroll click (USB HID standard).
            // Negate because EI direction is opposite to rdev/CGEvent conventions.
            let dx = -(evt.discrete_dx as f64) / 120.0;
            let dy = -(evt.discrete_dy as f64) / 120.0;
            try_emit(tx, InputEvent::Scroll { dx, dy }, telemetry);
        }

        EiEvent::KeyboardKey(evt) if active.is_some() => {
            use reis::ei::keyboard::KeyState;
            if let Some(hid) = wayflow_core::keymap::evdev::evdev_to_hid(evt.key) {
                let pressed = evt.state == KeyState::Press;
                // Update held_keys AFTER successful emit so that if the
                // emit drops (channel closed during shutdown), the
                // deactivation flush still has a record to release.
                emit_blocking(
                    tx,
                    InputEvent::Key {
                        keycode: hid,
                        pressed,
                        modifiers: *modifiers,
                    },
                    telemetry,
                )
                .await;
                if pressed {
                    held_keys.insert(hid);
                } else {
                    held_keys.remove(&hid);
                }
            } else {
                debug!("evdev key {} has no HID mapping, skipping", evt.key);
            }
        }

        EiEvent::KeyboardModifiers(evt) => {
            let new_mods = xkb_mods_to_proto(evt.depressed);
            let old_mods = *modifiers;
            *modifiers = new_mods;
            for (hid, pressed) in
                modifier_diff_actions(old_mods, new_mods, held_keys, active.is_some())
            {
                debug!(
                    "modifier diff reconcile: {:#x} {}",
                    hid,
                    if pressed { "press" } else { "release" }
                );
                if pressed {
                    held_keys.insert(hid);
                } else {
                    held_keys.remove(&hid);
                }
                emit_blocking(
                    tx,
                    InputEvent::Key {
                        keycode: hid,
                        pressed,
                        modifiers: new_mods,
                    },
                    telemetry,
                )
                .await;
            }
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

pub(super) fn evdev_to_mouse_button(code: u32) -> MouseButton {
    match code {
        0x110 => MouseButton::Left,
        0x111 => MouseButton::Right,
        0x112 => MouseButton::Middle,
        0x113 => MouseButton::Back,
        0x114 => MouseButton::Forward,
        n => MouseButton::Other((n & 0xff) as u8),
    }
}

pub(super) fn xkb_mods_to_proto(depressed: u32) -> Modifiers {
    Modifiers {
        shift: (depressed & 0x01) != 0,
        ctrl: (depressed & 0x04) != 0,
        alt: (depressed & 0x08) != 0,
        meta: (depressed & 0x40) != 0,
    }
}

/// Nudge the cursor 5px into the region it is closest to.
/// Applies x and y corrections simultaneously so a corner position
/// (e.g. at both the left and top barriers) does not end up at a second barrier.
fn nudge_inside(pos: (f64, f64), regions: &[Region]) -> (f64, f64) {
    const MARGIN: f64 = 5.0;
    for r in regions {
        let rx = r.x_offset() as f64;
        let ry = r.y_offset() as f64;
        let rx2 = rx + r.width() as f64 - 1.0;
        let ry2 = ry + r.height() as f64 - 1.0;
        // Accept this region if pos is within MARGIN of its bounding box.
        if pos.0 < rx - MARGIN || pos.0 > rx2 + MARGIN {
            continue;
        }
        if pos.1 < ry - MARGIN || pos.1 > ry2 + MARGIN {
            continue;
        }
        let (mut x, mut y) = pos;
        if x <= rx {
            x = rx + MARGIN;
        }
        if x >= rx2 {
            x = rx2 - MARGIN;
        }
        if y <= ry {
            y = ry + MARGIN;
        }
        if y >= ry2 {
            y = ry2 - MARGIN;
        }
        return (x, y);
    }
    pos
}

/// Nudge the cursor 5px away from the barrier that triggered capture,
/// using the known edge direction instead of guessing from position.
fn nudge_by_dir(pos: (f64, f64), dir: NudgeDir) -> (f64, f64) {
    const MARGIN: f64 = 5.0;
    match dir {
        NudgeDir::Left => (pos.0 + MARGIN, pos.1),
        NudgeDir::Right => (pos.0 - MARGIN, pos.1),
        NudgeDir::Top => (pos.0, pos.1 + MARGIN),
        NudgeDir::Bottom => (pos.0, pos.1 - MARGIN),
    }
}

/// Resolve which barrier was triggered and return its NudgeDir, if the
/// compositor provided the barrier_id in the Activated event.
fn nudge_dir_for_activation(evt: &Activated, dir_map: &HashMap<u32, NudgeDir>) -> Option<NudgeDir> {
    match evt.barrier_id()? {
        ActivatedBarrier::Barrier(id) => dir_map.get(&id.get()).copied(),
        ActivatedBarrier::UnknownBarrier => None,
    }
}

/// Returns true if `region` has another region directly adjacent on its right edge
/// (i.e., the two regions share an internal seam and no external barrier should be placed there).
fn has_right_neighbor(regions: &[Region], r: &Region) -> bool {
    let x2 = r.x_offset() + r.width() as i32;
    let ry = r.y_offset();
    let ry2 = ry + r.height() as i32;
    regions
        .iter()
        .any(|o| o.x_offset() == x2 && o.y_offset() < ry2 && o.y_offset() + o.height() as i32 > ry)
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
    regions
        .iter()
        .any(|o| o.y_offset() == y2 && o.x_offset() < rx2 && o.x_offset() + o.width() as i32 > rx)
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

fn build_barriers(regions: &[Region]) -> (Vec<Barrier>, HashMap<u32, NudgeDir>) {
    let mut barriers = Vec::with_capacity(regions.len() * 4);
    let mut dir_map: HashMap<u32, NudgeDir> = HashMap::new();
    let mut id: u32 = 1;
    let nz = |n: u32| -> BarrierID { NonZeroU32::new(n).unwrap() };
    for region in regions {
        let x = region.x_offset();
        let y = region.y_offset();
        let x2 = x + region.width() as i32;
        let y2 = y + region.height() as i32;
        // Only place a barrier on edges that are the outer boundary of the desktop.
        // Skip edges where an adjacent monitor forms an internal seam.
        if !has_right_neighbor(regions, region) {
            barriers.push(Barrier::new(nz(id), (x2, y, x2, y2)));
            dir_map.insert(id, NudgeDir::Right);
            id += 1;
        }
        if !has_left_neighbor(regions, region) {
            barriers.push(Barrier::new(nz(id), (x, y, x, y2)));
            dir_map.insert(id, NudgeDir::Left);
            id += 1;
        }
        if !has_bottom_neighbor(regions, region) {
            barriers.push(Barrier::new(nz(id), (x, y2, x2, y2)));
            dir_map.insert(id, NudgeDir::Bottom);
            id += 1;
        }
        if !has_top_neighbor(regions, region) {
            barriers.push(Barrier::new(nz(id), (x, y, x2, y)));
            dir_map.insert(id, NudgeDir::Top);
            id += 1;
        }
    }
    (barriers, dir_map)
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
        assert!(matches!(
            evdev_to_mouse_button(0x115),
            MouseButton::Other(_)
        ));
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
        let (barriers, dir_map) = build_barriers(&[]);
        assert!(barriers.is_empty());
        assert!(dir_map.is_empty());
    }

    fn shift_only() -> Modifiers {
        Modifiers { shift: true, ctrl: false, alt: false, meta: false }
    }
    fn no_mods() -> Modifiers {
        Modifiers::default()
    }

    #[test]
    fn diff_releases_left_synth_when_right_was_physical() {
        // Activation synthesised LeftShift; user releases physical RightShift.
        // EI bitmap drops shift; we should emit a release for LeftShift.
        let mut held = std::collections::HashSet::new();
        held.insert(HID_LEFT_SHIFT);
        let actions = modifier_diff_actions(shift_only(), no_mods(), &held, true);
        assert_eq!(actions, vec![(HID_LEFT_SHIFT, false)]);
    }

    #[test]
    fn diff_releases_right_when_only_right_held() {
        // Pathological case: only RightShift was tracked (e.g. via KeyboardKey
        // before bitmap fired). Bitmap drops shift; release the R variant.
        let mut held = std::collections::HashSet::new();
        held.insert(HID_RIGHT_SHIFT);
        let actions = modifier_diff_actions(shift_only(), no_mods(), &held, true);
        assert_eq!(actions, vec![(HID_RIGHT_SHIFT, false)]);
    }

    #[test]
    fn diff_no_release_when_neither_variant_held() {
        // Bitmap drops shift but neither L nor R is tracked -- the matching
        // KeyboardKey release already cleared held_keys, nothing to do.
        let held = std::collections::HashSet::new();
        let actions = modifier_diff_actions(shift_only(), no_mods(), &held, true);
        assert!(actions.is_empty());
    }

    #[test]
    fn diff_synths_press_when_bitmap_only_compositor() {
        // 0 -> 1 transition with nothing in held_keys: synth a Left press
        // so the mac client sees the modifier go down.
        let held = std::collections::HashSet::new();
        let actions = modifier_diff_actions(no_mods(), shift_only(), &held, true);
        assert_eq!(actions, vec![(HID_LEFT_SHIFT, true)]);
    }

    #[test]
    fn diff_no_press_when_keyboardkey_already_tracked() {
        // 0 -> 1 with the L variant already tracked (KeyboardKey fired first).
        // No synth press needed.
        let mut held = std::collections::HashSet::new();
        held.insert(HID_LEFT_SHIFT);
        let actions = modifier_diff_actions(no_mods(), shift_only(), &held, true);
        assert!(actions.is_empty());
    }

    #[test]
    fn diff_no_press_when_right_already_tracked() {
        // 0 -> 1 with the R variant tracked (user pressed right shift mid-session).
        // No synth press for L.
        let mut held = std::collections::HashSet::new();
        held.insert(HID_RIGHT_SHIFT);
        let actions = modifier_diff_actions(no_mods(), shift_only(), &held, true);
        assert!(actions.is_empty());
    }

    #[test]
    fn diff_skips_when_inactive() {
        let mut held = std::collections::HashSet::new();
        held.insert(HID_LEFT_SHIFT);
        let actions = modifier_diff_actions(shift_only(), no_mods(), &held, false);
        assert!(actions.is_empty());
    }

    #[test]
    fn diff_handles_all_modifiers_at_once() {
        let mut held = std::collections::HashSet::new();
        held.insert(HID_LEFT_SHIFT);
        held.insert(HID_RIGHT_CTRL);
        held.insert(HID_LEFT_ALT);
        held.insert(HID_LEFT_META);
        let all = Modifiers { shift: true, ctrl: true, alt: true, meta: true };
        let actions = modifier_diff_actions(all, no_mods(), &held, true);
        assert_eq!(
            actions,
            vec![
                (HID_LEFT_SHIFT, false),
                (HID_RIGHT_CTRL, false),
                (HID_LEFT_ALT, false),
                (HID_LEFT_META, false),
            ]
        );
    }
}
