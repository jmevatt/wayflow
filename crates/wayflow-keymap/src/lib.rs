//! Keycode translation between platform namespaces.
//!
//! No platform dependencies on purpose: the tables are pure data, so they compile and
//! test on any machine rather than only on the one they describe.

mod mac;

/// macOS `CGEventFlags` bits that matter for injected keys.
pub mod flags {
    pub const SHIFT: u64 = 0x0002_0000;
    pub const CONTROL: u64 = 0x0004_0000;
    pub const ALTERNATE: u64 = 0x0008_0000;
    pub const COMMAND: u64 = 0x0010_0000;
    pub const ALPHA_SHIFT: u64 = 0x0001_0000;
}

/// Translate a Linux evdev key code to a macOS virtual keycode.
///
/// Returns `None` for keys with no macOS equivalent, which the caller should drop rather
/// than substitute: pressing an arbitrary wrong key is worse than pressing none.
#[must_use]
pub fn evdev_to_mac(evdev: u16) -> Option<u16> {
    mac::EVDEV_TO_MAC
        .binary_search_by_key(&evdev, |&(k, _)| k)
        .ok()
        .map(|i| mac::EVDEV_TO_MAC[i].1)
}

/// Whether this evdev code is a modifier macOS tracks through event flags.
#[must_use]
pub fn is_modifier(evdev: u16) -> bool {
    mac::MODIFIER_EVDEV.contains(&evdev)
}

/// The `CGEventFlags` bit a modifier key contributes, if any.
#[must_use]
pub fn mac_flag_for(evdev: u16) -> Option<u64> {
    Some(match evdev {
        42 | 54 => flags::SHIFT,
        29 | 97 => flags::CONTROL,
        56 | 100 => flags::ALTERNATE,
        125 | 126 => flags::COMMAND,
        58 => flags::ALPHA_SHIFT,
        _ => return None,
    })
}

/// Tracks which modifiers are physically held so injected events carry the right flags.
///
/// macOS will not infer this. Posting the `A` keycode while Shift is down produces a
/// lowercase `a` unless the event itself carries the shift flag, so the client has to
/// remember what the host is holding.
#[derive(Debug, Default, Clone, Copy)]
pub struct ModifierState {
    held: u64,
}

impl ModifierState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a modifier transition. Non-modifier keys leave the state untouched.
    pub fn update(&mut self, evdev: u16, pressed: bool) {
        let Some(bit) = mac_flag_for(evdev) else {
            return;
        };
        if pressed {
            self.held |= bit;
        } else {
            self.held &= !bit;
        }
    }

    /// Current flag bits to stamp onto an outgoing event.
    #[must_use]
    pub fn flags(self) -> u64 {
        self.held
    }

    /// Drop every held modifier.
    ///
    /// Called when control leaves the client. Without it, releasing Shift on the host
    /// after the pointer has already gone home would never reach the client, and it would
    /// believe Shift was held forever.
    pub fn clear(&mut self) {
        self.held = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_sorted_and_unique() {
        // binary_search_by_key silently returns wrong answers on unsorted input, so the
        // ordering this relies on is asserted rather than assumed.
        let keys: Vec<u16> = mac::EVDEV_TO_MAC.iter().map(|&(k, _)| k).collect();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(keys, sorted, "evdev keys must be sorted and unique");
    }

    #[test]
    fn letters_map_to_their_mac_positions() {
        assert_eq!(evdev_to_mac(30), Some(0x00)); // A
        assert_eq!(evdev_to_mac(17), Some(0x0D)); // W
        assert_eq!(evdev_to_mac(44), Some(0x06)); // Z
    }

    #[test]
    fn mac_digit_order_is_not_sequential() {
        // The one part of this table nobody guesses right: on macOS 5 is 0x17 and 6 is
        // 0x16, so the digits are not in ascending order. Getting it wrong swaps them.
        assert_eq!(evdev_to_mac(6), Some(0x17)); // KEY_5
        assert_eq!(evdev_to_mac(7), Some(0x16)); // KEY_6
    }

    #[test]
    fn unmapped_keys_are_dropped_not_guessed() {
        assert_eq!(evdev_to_mac(0), None);
        assert_eq!(evdev_to_mac(9999), None);
    }

    #[test]
    fn every_mapped_key_has_a_plausible_mac_code() {
        // macOS virtual keycodes are 7-bit. A value above 0x7F means a typo in the table.
        for &(evdev, mac) in mac::EVDEV_TO_MAC {
            assert!(mac <= 0x7F, "evdev {evdev} maps to out-of-range {mac:#x}");
        }
    }

    #[test]
    fn modifier_state_tracks_press_and_release() {
        let mut mods = ModifierState::new();
        assert_eq!(mods.flags(), 0);
        mods.update(42, true); // LEFTSHIFT
        assert_eq!(mods.flags(), flags::SHIFT);
        mods.update(29, true); // LEFTCTRL
        assert_eq!(mods.flags(), flags::SHIFT | flags::CONTROL);
        mods.update(42, false);
        assert_eq!(mods.flags(), flags::CONTROL);
    }

    #[test]
    fn either_shift_releases_the_same_bit() {
        // Press left shift, release right shift. A naive per-key bitmap would leave shift
        // stuck on; the flag is shared, so releasing either clears it. This is the
        // stuck-modifier bug that plagues this class of software.
        let mut mods = ModifierState::new();
        mods.update(42, true);
        mods.update(54, false);
        assert_eq!(mods.flags(), 0, "shift should not be stuck");
    }

    #[test]
    fn non_modifiers_do_not_disturb_state() {
        let mut mods = ModifierState::new();
        mods.update(42, true);
        mods.update(30, true); // A
        mods.update(30, false);
        assert_eq!(mods.flags(), flags::SHIFT);
    }

    #[test]
    fn clear_drops_everything() {
        let mut mods = ModifierState::new();
        mods.update(125, true);
        mods.update(42, true);
        mods.clear();
        assert_eq!(mods.flags(), 0);
    }
}
