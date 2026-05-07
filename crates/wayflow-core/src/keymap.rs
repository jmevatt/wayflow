// USB HID keyboard usage page (0x07) <-> platform keycode conversions.
//
// All platforms send HID usage codes on the wire. Each backend translates
// to/from its native representation:
//   Linux: evdev key codes (drivers/hid/hid-input.c hid_keyboard[] table)
//   macOS/Windows: rdev::Key enum (CGEventPost / SendInput)

// ---- Linux evdev <-> HID -----------------------------------------------

#[cfg(target_os = "linux")]
pub mod evdev {
    // HID keyboard usage page -> Linux evdev key code.
    // Index = HID usage (0x00..=0xFF), value = evdev code (0 = no mapping).
    // Source: Linux kernel drivers/hid/hid-input.c hid_keyboard[].
    static HID_TO_EVDEV: [u16; 256] = [
        //  +0    +1    +2    +3    +4    +5    +6    +7    +8    +9    +A    +B    +C    +D    +E    +F
        0, 0, 0, 0, 30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, // 0x00
        50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45, 21, 44, 2, 3, // 0x10
        4, 5, 6, 7, 8, 9, 10, 11, 28, 1, 14, 15, 57, 12, 13, 26, // 0x20
        27, 43, 43, 39, 40, 41, 51, 52, 53, 58, 59, 60, 61, 62, 63, 64, // 0x30
        65, 66, 67, 68, 87, 88, 99, 70, 119, 110, 102, 104, 111, 107, 109, 106, // 0x40
        105, 108, 103, 69, 98, 55, 74, 78, 96, 79, 80, 81, 75, 76, 77, 71, // 0x50
        72, 73, 82, 83, 86, 127, 116, 117, 183, 184, 185, 186, 187, 188, 189, 190, // 0x60
        191, 192, 193, 194, 134, 138, 130, 132, 128, 129, 131, 137, 133, 135, 136,
        113, // 0x70
        115, 114, 0, 0, 0, 121, 0, 89, 93, 124, 92, 94, 95, 0, 0, 0, // 0x80
        122, 123, 90, 91, 85, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // 0x90
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // 0xA0
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // 0xB0
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // 0xC0
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // 0xD0
        29, 42, 56, 125, 97, 54, 100, 126, 0, 0, 0, 0, 0, 0, 0, 0, // 0xE0
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // 0xF0
    ];

    pub fn hid_to_evdev(hid: u32) -> Option<u32> {
        let v = *HID_TO_EVDEV.get(hid as usize)?;
        if v == 0 {
            None
        } else {
            Some(v as u32)
        }
    }

    pub fn evdev_to_hid(evdev: u32) -> Option<u32> {
        HID_TO_EVDEV
            .iter()
            .position(|&e| e != 0 && e as u32 == evdev)
            .map(|i| i as u32)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn key_a_roundtrip() {
            // HID 0x04 <-> evdev KEY_A (30)
            assert_eq!(hid_to_evdev(0x04), Some(30));
            assert_eq!(evdev_to_hid(30), Some(0x04));
        }

        #[test]
        fn enter_roundtrip() {
            assert_eq!(hid_to_evdev(0x28), Some(28));
            assert_eq!(evdev_to_hid(28), Some(0x28));
        }

        #[test]
        fn escape_roundtrip() {
            assert_eq!(hid_to_evdev(0x29), Some(1));
            assert_eq!(evdev_to_hid(1), Some(0x29));
        }

        #[test]
        fn f1_roundtrip() {
            assert_eq!(hid_to_evdev(0x3A), Some(59));
            assert_eq!(evdev_to_hid(59), Some(0x3A));
        }

        #[test]
        fn f12_roundtrip() {
            assert_eq!(hid_to_evdev(0x45), Some(88));
            assert_eq!(evdev_to_hid(88), Some(0x45));
        }

        #[test]
        fn arrow_keys_roundtrip() {
            assert_eq!(hid_to_evdev(0x4F), Some(106)); // right
            assert_eq!(hid_to_evdev(0x50), Some(105)); // left
            assert_eq!(hid_to_evdev(0x51), Some(108)); // down
            assert_eq!(hid_to_evdev(0x52), Some(103)); // up
        }

        #[test]
        fn left_ctrl_roundtrip() {
            assert_eq!(hid_to_evdev(0xE0), Some(29));
            assert_eq!(evdev_to_hid(29), Some(0xE0));
        }

        #[test]
        fn left_shift_roundtrip() {
            assert_eq!(hid_to_evdev(0xE1), Some(42));
            assert_eq!(evdev_to_hid(42), Some(0xE1));
        }

        #[test]
        fn left_meta_roundtrip() {
            assert_eq!(hid_to_evdev(0xE3), Some(125));
            assert_eq!(evdev_to_hid(125), Some(0xE3));
        }

        #[test]
        fn right_alt_roundtrip() {
            assert_eq!(hid_to_evdev(0xE6), Some(100));
            assert_eq!(evdev_to_hid(100), Some(0xE6));
        }

        #[test]
        fn unmapped_hid_returns_none() {
            assert_eq!(hid_to_evdev(0x00), None);
            assert_eq!(hid_to_evdev(0x01), None);
            assert_eq!(hid_to_evdev(0xA0), None);
            assert_eq!(hid_to_evdev(0xFF), None);
        }

        #[test]
        fn unmapped_evdev_returns_none() {
            assert_eq!(evdev_to_hid(0), None);
            assert_eq!(evdev_to_hid(255), None);
        }

        #[test]
        fn hid_out_of_range_returns_none() {
            assert_eq!(hid_to_evdev(256), None);
            assert_eq!(hid_to_evdev(u32::MAX), None);
        }

        #[test]
        fn numpad_roundtrip() {
            assert_eq!(hid_to_evdev(0x59), Some(79)); // KP1
            assert_eq!(hid_to_evdev(0x62), Some(82)); // KP0
            assert_eq!(hid_to_evdev(0x63), Some(83)); // KP decimal
        }

        #[test]
        fn space_roundtrip() {
            assert_eq!(hid_to_evdev(0x2C), Some(57));
            assert_eq!(evdev_to_hid(57), Some(0x2C));
        }
    }
}

// ---- macOS / Windows rdev::Key <-> HID ---------------------------------

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub mod rdev_keys {
    use rdev::Key;

    pub fn rdev_to_hid(key: Key) -> Option<u32> {
        match key {
            Key::KeyA => Some(0x04),
            Key::KeyB => Some(0x05),
            Key::KeyC => Some(0x06),
            Key::KeyD => Some(0x07),
            Key::KeyE => Some(0x08),
            Key::KeyF => Some(0x09),
            Key::KeyG => Some(0x0A),
            Key::KeyH => Some(0x0B),
            Key::KeyI => Some(0x0C),
            Key::KeyJ => Some(0x0D),
            Key::KeyK => Some(0x0E),
            Key::KeyL => Some(0x0F),
            Key::KeyM => Some(0x10),
            Key::KeyN => Some(0x11),
            Key::KeyO => Some(0x12),
            Key::KeyP => Some(0x13),
            Key::KeyQ => Some(0x14),
            Key::KeyR => Some(0x15),
            Key::KeyS => Some(0x16),
            Key::KeyT => Some(0x17),
            Key::KeyU => Some(0x18),
            Key::KeyV => Some(0x19),
            Key::KeyW => Some(0x1A),
            Key::KeyX => Some(0x1B),
            Key::KeyY => Some(0x1C),
            Key::KeyZ => Some(0x1D),
            Key::Num1 => Some(0x1E),
            Key::Num2 => Some(0x1F),
            Key::Num3 => Some(0x20),
            Key::Num4 => Some(0x21),
            Key::Num5 => Some(0x22),
            Key::Num6 => Some(0x23),
            Key::Num7 => Some(0x24),
            Key::Num8 => Some(0x25),
            Key::Num9 => Some(0x26),
            Key::Num0 => Some(0x27),
            Key::Return => Some(0x28),
            Key::Escape => Some(0x29),
            Key::Backspace => Some(0x2A),
            Key::Tab => Some(0x2B),
            Key::Space => Some(0x2C),
            Key::Minus => Some(0x2D),
            Key::Equal => Some(0x2E),
            Key::LeftBracket => Some(0x2F),
            Key::RightBracket => Some(0x30),
            Key::BackSlash => Some(0x31),
            Key::IntlBackslash => Some(0x64), // ISO extra key (KEY_102ND)
            Key::SemiColon => Some(0x33),
            Key::Quote => Some(0x34),
            Key::BackQuote => Some(0x35),
            Key::Comma => Some(0x36),
            Key::Dot => Some(0x37),
            Key::Slash => Some(0x38),
            Key::CapsLock => Some(0x39),
            Key::F1 => Some(0x3A),
            Key::F2 => Some(0x3B),
            Key::F3 => Some(0x3C),
            Key::F4 => Some(0x3D),
            Key::F5 => Some(0x3E),
            Key::F6 => Some(0x3F),
            Key::F7 => Some(0x40),
            Key::F8 => Some(0x41),
            Key::F9 => Some(0x42),
            Key::F10 => Some(0x43),
            Key::F11 => Some(0x44),
            Key::F12 => Some(0x45),
            Key::PrintScreen => Some(0x46),
            Key::ScrollLock => Some(0x47),
            Key::Pause => Some(0x48),
            Key::Insert => Some(0x49),
            Key::Home => Some(0x4A),
            Key::PageUp => Some(0x4B),
            Key::Delete => Some(0x4C),
            Key::End => Some(0x4D),
            Key::PageDown => Some(0x4E),
            Key::RightArrow => Some(0x4F),
            Key::LeftArrow => Some(0x50),
            Key::DownArrow => Some(0x51),
            Key::UpArrow => Some(0x52),
            Key::NumLock => Some(0x53),
            Key::KpDivide => Some(0x54),
            Key::KpMultiply => Some(0x55),
            Key::KpMinus => Some(0x56),
            Key::KpPlus => Some(0x57),
            Key::KpReturn => Some(0x58),
            Key::Kp1 => Some(0x59),
            Key::Kp2 => Some(0x5A),
            Key::Kp3 => Some(0x5B),
            Key::Kp4 => Some(0x5C),
            Key::Kp5 => Some(0x5D),
            Key::Kp6 => Some(0x5E),
            Key::Kp7 => Some(0x5F),
            Key::Kp8 => Some(0x60),
            Key::Kp9 => Some(0x61),
            Key::Kp0 => Some(0x62),
            Key::KpDelete => Some(0x63),
            Key::ControlLeft => Some(0xE0),
            Key::ShiftLeft => Some(0xE1),
            Key::Alt => Some(0xE2),
            Key::MetaLeft => Some(0xE3),
            Key::ControlRight => Some(0xE4),
            Key::ShiftRight => Some(0xE5),
            Key::AltGr => Some(0xE6),
            Key::MetaRight => Some(0xE7),
            Key::Function | Key::Unknown(_) => None,
        }
    }

    pub fn hid_to_rdev(hid: u32) -> Option<Key> {
        match hid {
            0x04 => Some(Key::KeyA),
            0x05 => Some(Key::KeyB),
            0x06 => Some(Key::KeyC),
            0x07 => Some(Key::KeyD),
            0x08 => Some(Key::KeyE),
            0x09 => Some(Key::KeyF),
            0x0A => Some(Key::KeyG),
            0x0B => Some(Key::KeyH),
            0x0C => Some(Key::KeyI),
            0x0D => Some(Key::KeyJ),
            0x0E => Some(Key::KeyK),
            0x0F => Some(Key::KeyL),
            0x10 => Some(Key::KeyM),
            0x11 => Some(Key::KeyN),
            0x12 => Some(Key::KeyO),
            0x13 => Some(Key::KeyP),
            0x14 => Some(Key::KeyQ),
            0x15 => Some(Key::KeyR),
            0x16 => Some(Key::KeyS),
            0x17 => Some(Key::KeyT),
            0x18 => Some(Key::KeyU),
            0x19 => Some(Key::KeyV),
            0x1A => Some(Key::KeyW),
            0x1B => Some(Key::KeyX),
            0x1C => Some(Key::KeyY),
            0x1D => Some(Key::KeyZ),
            0x1E => Some(Key::Num1),
            0x1F => Some(Key::Num2),
            0x20 => Some(Key::Num3),
            0x21 => Some(Key::Num4),
            0x22 => Some(Key::Num5),
            0x23 => Some(Key::Num6),
            0x24 => Some(Key::Num7),
            0x25 => Some(Key::Num8),
            0x26 => Some(Key::Num9),
            0x27 => Some(Key::Num0),
            0x28 => Some(Key::Return),
            0x29 => Some(Key::Escape),
            0x2A => Some(Key::Backspace),
            0x2B => Some(Key::Tab),
            0x2C => Some(Key::Space),
            0x2D => Some(Key::Minus),
            0x2E => Some(Key::Equal),
            0x2F => Some(Key::LeftBracket),
            0x30 => Some(Key::RightBracket),
            0x31 | 0x32 => Some(Key::BackSlash),
            0x33 => Some(Key::SemiColon),
            0x34 => Some(Key::Quote),
            0x35 => Some(Key::BackQuote),
            0x36 => Some(Key::Comma),
            0x37 => Some(Key::Dot),
            0x38 => Some(Key::Slash),
            0x39 => Some(Key::CapsLock),
            0x3A => Some(Key::F1),
            0x3B => Some(Key::F2),
            0x3C => Some(Key::F3),
            0x3D => Some(Key::F4),
            0x3E => Some(Key::F5),
            0x3F => Some(Key::F6),
            0x40 => Some(Key::F7),
            0x41 => Some(Key::F8),
            0x42 => Some(Key::F9),
            0x43 => Some(Key::F10),
            0x44 => Some(Key::F11),
            0x45 => Some(Key::F12),
            0x46 => Some(Key::PrintScreen),
            0x47 => Some(Key::ScrollLock),
            0x48 => Some(Key::Pause),
            0x49 => Some(Key::Insert),
            0x4A => Some(Key::Home),
            0x4B => Some(Key::PageUp),
            0x4C => Some(Key::Delete),
            0x4D => Some(Key::End),
            0x4E => Some(Key::PageDown),
            0x4F => Some(Key::RightArrow),
            0x50 => Some(Key::LeftArrow),
            0x51 => Some(Key::DownArrow),
            0x52 => Some(Key::UpArrow),
            0x53 => Some(Key::NumLock),
            0x54 => Some(Key::KpDivide),
            0x55 => Some(Key::KpMultiply),
            0x56 => Some(Key::KpMinus),
            0x57 => Some(Key::KpPlus),
            0x58 => Some(Key::KpReturn),
            0x59 => Some(Key::Kp1),
            0x5A => Some(Key::Kp2),
            0x5B => Some(Key::Kp3),
            0x5C => Some(Key::Kp4),
            0x5D => Some(Key::Kp5),
            0x5E => Some(Key::Kp6),
            0x5F => Some(Key::Kp7),
            0x60 => Some(Key::Kp8),
            0x61 => Some(Key::Kp9),
            0x62 => Some(Key::Kp0),
            0x63 => Some(Key::KpDelete),
            0x64 => Some(Key::IntlBackslash),
            0xE0 => Some(Key::ControlLeft),
            0xE1 => Some(Key::ShiftLeft),
            0xE2 => Some(Key::Alt),
            0xE3 => Some(Key::MetaLeft),
            0xE4 => Some(Key::ControlRight),
            0xE5 => Some(Key::ShiftRight),
            0xE6 => Some(Key::AltGr),
            0xE7 => Some(Key::MetaRight),
            _ => None,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use rdev::Key;

        #[test]
        fn key_a_roundtrip() {
            assert_eq!(rdev_to_hid(Key::KeyA), Some(0x04));
            assert_eq!(hid_to_rdev(0x04), Some(Key::KeyA));
        }

        #[test]
        fn all_letters_roundtrip() {
            let letters = [
                Key::KeyA,
                Key::KeyB,
                Key::KeyC,
                Key::KeyD,
                Key::KeyE,
                Key::KeyF,
                Key::KeyG,
                Key::KeyH,
                Key::KeyI,
                Key::KeyJ,
                Key::KeyK,
                Key::KeyL,
                Key::KeyM,
                Key::KeyN,
                Key::KeyO,
                Key::KeyP,
                Key::KeyQ,
                Key::KeyR,
                Key::KeyS,
                Key::KeyT,
                Key::KeyU,
                Key::KeyV,
                Key::KeyW,
                Key::KeyX,
                Key::KeyY,
                Key::KeyZ,
            ];
            for key in letters {
                let hid = rdev_to_hid(key).unwrap_or_else(|| panic!("{key:?} missing"));
                let back = hid_to_rdev(hid).unwrap_or_else(|| panic!("hid {hid:#x} missing"));
                assert_eq!(back, key, "roundtrip failed for {key:?}");
            }
        }

        #[test]
        fn digits_roundtrip() {
            for key in [
                Key::Num1,
                Key::Num2,
                Key::Num3,
                Key::Num4,
                Key::Num5,
                Key::Num6,
                Key::Num7,
                Key::Num8,
                Key::Num9,
                Key::Num0,
            ] {
                let hid = rdev_to_hid(key).unwrap();
                assert_eq!(hid_to_rdev(hid), Some(key));
            }
        }

        #[test]
        fn function_keys_roundtrip() {
            for key in [
                Key::F1,
                Key::F2,
                Key::F3,
                Key::F4,
                Key::F5,
                Key::F6,
                Key::F7,
                Key::F8,
                Key::F9,
                Key::F10,
                Key::F11,
                Key::F12,
            ] {
                let hid = rdev_to_hid(key).unwrap();
                assert_eq!(hid_to_rdev(hid), Some(key));
            }
        }

        #[test]
        fn modifiers_roundtrip() {
            for key in [
                Key::ControlLeft,
                Key::ControlRight,
                Key::ShiftLeft,
                Key::ShiftRight,
                Key::Alt,
                Key::AltGr,
                Key::MetaLeft,
                Key::MetaRight,
            ] {
                let hid = rdev_to_hid(key).unwrap();
                assert_eq!(hid_to_rdev(hid), Some(key));
            }
        }

        #[test]
        fn nav_keys_roundtrip() {
            for key in [
                Key::UpArrow,
                Key::DownArrow,
                Key::LeftArrow,
                Key::RightArrow,
                Key::Home,
                Key::End,
                Key::PageUp,
                Key::PageDown,
                Key::Insert,
                Key::Delete,
            ] {
                let hid = rdev_to_hid(key).unwrap();
                assert_eq!(hid_to_rdev(hid), Some(key));
            }
        }

        #[test]
        fn numpad_roundtrip() {
            for key in [
                Key::Kp0,
                Key::Kp1,
                Key::Kp2,
                Key::Kp3,
                Key::Kp4,
                Key::Kp5,
                Key::Kp6,
                Key::Kp7,
                Key::Kp8,
                Key::Kp9,
                Key::KpReturn,
                Key::KpPlus,
                Key::KpMinus,
                Key::KpMultiply,
                Key::KpDivide,
                Key::KpDelete,
            ] {
                let hid = rdev_to_hid(key).unwrap();
                assert_eq!(hid_to_rdev(hid), Some(key));
            }
        }

        #[test]
        fn function_key_returns_none() {
            assert_eq!(rdev_to_hid(Key::Function), None);
        }

        #[test]
        fn unknown_key_returns_none() {
            assert_eq!(rdev_to_hid(Key::Unknown(0xDEAD)), None);
        }

        #[test]
        fn unmapped_hid_returns_none() {
            assert_eq!(hid_to_rdev(0x00), None);
            assert_eq!(hid_to_rdev(0xA0), None);
            assert_eq!(hid_to_rdev(0xFF), None);
            assert_eq!(hid_to_rdev(u32::MAX), None);
        }

        #[test]
        fn intl_backslash_maps_to_0x64() {
            assert_eq!(rdev_to_hid(Key::IntlBackslash), Some(0x64));
            assert_eq!(hid_to_rdev(0x64), Some(Key::IntlBackslash));
        }

        #[test]
        fn hid_0x32_maps_to_backslash() {
            // Non-US #/~ key: no distinct rdev variant, falls back to BackSlash
            assert_eq!(hid_to_rdev(0x32), Some(Key::BackSlash));
        }
    }
}

/// HID Usage ID (USB HID Keyboard/Keypad page) -> macOS CG virtual keycode.
/// CG virtual keycodes are the integer values from `<HIToolbox/Events.h>` (the
/// `kVK_*` constants). Used by the macOS client backend to bypass rdev's
/// stateful key-event path -- rdev tracks modifier flags internally and the
/// state can drift over a long session, producing "stuck modifier" symptoms.
///
/// Returns `None` for HID codes that have no direct mac equivalent (NumLock,
/// Insert, etc.); the caller should fall back to rdev for those.
pub fn hid_to_cg_keycode(hid: u32) -> Option<u16> {
    Some(match hid {
        // Letters
        0x04 => 0x00, // A
        0x05 => 0x0B, // B
        0x06 => 0x08, // C
        0x07 => 0x02, // D
        0x08 => 0x0E, // E
        0x09 => 0x03, // F
        0x0A => 0x05, // G
        0x0B => 0x04, // H
        0x0C => 0x22, // I
        0x0D => 0x26, // J
        0x0E => 0x28, // K
        0x0F => 0x25, // L
        0x10 => 0x2E, // M
        0x11 => 0x2D, // N
        0x12 => 0x1F, // O
        0x13 => 0x23, // P
        0x14 => 0x0C, // Q
        0x15 => 0x0F, // R
        0x16 => 0x01, // S
        0x17 => 0x11, // T
        0x18 => 0x20, // U
        0x19 => 0x09, // V
        0x1A => 0x0D, // W
        0x1B => 0x07, // X
        0x1C => 0x10, // Y
        0x1D => 0x06, // Z
        // Number row
        0x1E => 0x12, // 1
        0x1F => 0x13, // 2
        0x20 => 0x14, // 3
        0x21 => 0x15, // 4
        0x22 => 0x17, // 5
        0x23 => 0x16, // 6
        0x24 => 0x1A, // 7
        0x25 => 0x1C, // 8
        0x26 => 0x19, // 9
        0x27 => 0x1D, // 0
        // Whitespace + control
        0x28 => 0x24, // Return
        0x29 => 0x35, // Escape
        0x2A => 0x33, // Backspace (kVK_Delete)
        0x2B => 0x30, // Tab
        0x2C => 0x31, // Space
        // Punctuation
        0x2D => 0x1B, // -
        0x2E => 0x18, // =
        0x2F => 0x21, // [
        0x30 => 0x1E, // ]
        0x31 => 0x2A, // backslash
        0x33 => 0x29, // ;
        0x34 => 0x27, // '
        0x35 => 0x32, // `
        0x36 => 0x2B, // ,
        0x37 => 0x2F, // .
        0x38 => 0x2C, // /
        0x39 => 0x39, // CapsLock
        // F-keys
        0x3A => 0x7A, // F1
        0x3B => 0x78, // F2
        0x3C => 0x63, // F3
        0x3D => 0x76, // F4
        0x3E => 0x60, // F5
        0x3F => 0x61, // F6
        0x40 => 0x62, // F7
        0x41 => 0x64, // F8
        0x42 => 0x65, // F9
        0x43 => 0x6D, // F10
        0x44 => 0x67, // F11
        0x45 => 0x6F, // F12
        // Editing / navigation
        0x4A => 0x73, // Home
        0x4B => 0x74, // PageUp
        0x4C => 0x75, // ForwardDelete
        0x4D => 0x77, // End
        0x4E => 0x79, // PageDown
        0x4F => 0x7C, // Right
        0x50 => 0x7B, // Left
        0x51 => 0x7D, // Down
        0x52 => 0x7E, // Up
        // Numpad
        0x54 => 0x4B, // Kp /
        0x55 => 0x43, // Kp *
        0x56 => 0x4E, // Kp -
        0x57 => 0x45, // Kp +
        0x58 => 0x4C, // Kp Enter
        0x59 => 0x53, // Kp 1
        0x5A => 0x54, // Kp 2
        0x5B => 0x55, // Kp 3
        0x5C => 0x56, // Kp 4
        0x5D => 0x57, // Kp 5
        0x5E => 0x58, // Kp 6
        0x5F => 0x59, // Kp 7
        0x60 => 0x5B, // Kp 8
        0x61 => 0x5C, // Kp 9
        0x62 => 0x52, // Kp 0
        0x63 => 0x41, // Kp .
        // Modifiers
        0xE0 => 0x3B, // LeftCtrl
        0xE1 => 0x38, // LeftShift
        0xE2 => 0x3A, // LeftAlt   -> Option
        0xE3 => 0x37, // LeftMeta  -> Command
        0xE4 => 0x3E, // RightCtrl
        0xE5 => 0x3C, // RightShift
        0xE6 => 0x3D, // RightAlt  -> RightOption
        0xE7 => 0x36, // RightMeta -> RightCommand
        _ => return None,
    })
}

#[cfg(test)]
mod cg_tests {
    use super::*;

    #[test]
    fn modifiers_have_cg_codes() {
        assert_eq!(hid_to_cg_keycode(0xE0), Some(0x3B)); // ctrl
        assert_eq!(hid_to_cg_keycode(0xE1), Some(0x38)); // shift
        assert_eq!(hid_to_cg_keycode(0xE2), Some(0x3A)); // alt -> option
        assert_eq!(hid_to_cg_keycode(0xE3), Some(0x37)); // meta -> command
    }

    #[test]
    fn alpha_have_cg_codes() {
        assert_eq!(hid_to_cg_keycode(0x04), Some(0x00)); // A
        assert_eq!(hid_to_cg_keycode(0x08), Some(0x0E)); // E
        assert_eq!(hid_to_cg_keycode(0x1D), Some(0x06)); // Z
    }

    #[test]
    fn unmapped_hid_returns_none() {
        assert_eq!(hid_to_cg_keycode(0x00), None);
        assert_eq!(hid_to_cg_keycode(0x53), None); // NumLock has no CG equivalent
        assert_eq!(hid_to_cg_keycode(0xFF), None);
    }
}
