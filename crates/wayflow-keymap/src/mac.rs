//! Linux evdev code to macOS virtual keycode.
//!
//! Both sides name physical key *positions*, not characters, which is what makes this
//! table possible at all: evdev `KEY_Q` and macOS `kVK_ANSI_Q` both mean "the key where Q
//! sits on a US board", and each OS applies its own layout on top. A table built from
//! characters would break the moment either machine used a non-US layout.
//!
//! macOS values are from Carbon `HIToolbox/Events.h` (`kVK_*`); Linux values from
//! `linux/input-event-codes.h`.

/// `(evdev, macOS)` pairs. Sorted by evdev code so the table reads in scancode order.
pub(crate) const EVDEV_TO_MAC: &[(u16, u16)] = &[
    (1, 0x35),   // ESC          -> Escape
    (2, 0x12),   // 1            -> ANSI_1
    (3, 0x13),   // 2
    (4, 0x14),   // 3
    (5, 0x15),   // 4
    (6, 0x17),   // 5            -> note: mac 5 is 0x17, 6 is 0x16 (not sequential)
    (7, 0x16),   // 6
    (8, 0x1A),   // 7
    (9, 0x1C),   // 8
    (10, 0x19),  // 9
    (11, 0x1D),  // 0
    (12, 0x1B),  // MINUS        -> ANSI_Minus
    (13, 0x18),  // EQUAL        -> ANSI_Equal
    (14, 0x33),  // BACKSPACE    -> Delete
    (15, 0x30),  // TAB
    (16, 0x0C),  // Q
    (17, 0x0D),  // W
    (18, 0x0E),  // E
    (19, 0x0F),  // R
    (20, 0x11),  // T
    (21, 0x10),  // Y
    (22, 0x20),  // U
    (23, 0x22),  // I
    (24, 0x1F),  // O
    (25, 0x23),  // P
    (26, 0x21),  // LEFTBRACE
    (27, 0x1E),  // RIGHTBRACE
    (28, 0x24),  // ENTER        -> Return
    (29, 0x3B),  // LEFTCTRL     -> Control
    (30, 0x00),  // A
    (31, 0x01),  // S
    (32, 0x02),  // D
    (33, 0x03),  // F
    (34, 0x05),  // G
    (35, 0x04),  // H
    (36, 0x26),  // J
    (37, 0x28),  // K
    (38, 0x25),  // L
    (39, 0x29),  // SEMICOLON
    (40, 0x27),  // APOSTROPHE   -> Quote
    (41, 0x32),  // GRAVE
    (42, 0x38),  // LEFTSHIFT    -> Shift
    (43, 0x2A),  // BACKSLASH
    (44, 0x06),  // Z
    (45, 0x07),  // X
    (46, 0x08),  // C
    (47, 0x09),  // V
    (48, 0x0B),  // B
    (49, 0x2D),  // N
    (50, 0x2E),  // M
    (51, 0x2B),  // COMMA
    (52, 0x2F),  // DOT          -> Period
    (53, 0x2C),  // SLASH
    (54, 0x3C),  // RIGHTSHIFT
    (55, 0x43),  // KPASTERISK   -> KeypadMultiply
    (56, 0x3A),  // LEFTALT      -> Option
    (57, 0x31),  // SPACE
    (58, 0x39),  // CAPSLOCK
    (59, 0x7A),  // F1
    (60, 0x78),  // F2
    (61, 0x63),  // F3
    (62, 0x76),  // F4
    (63, 0x60),  // F5
    (64, 0x61),  // F6
    (65, 0x62),  // F7
    (66, 0x64),  // F8
    (67, 0x65),  // F9
    (68, 0x6D),  // F10
    (69, 0x47),  // NUMLOCK      -> KeypadClear (mac has no numlock)
    (71, 0x59),  // KP7
    (72, 0x5B),  // KP8
    (73, 0x5C),  // KP9
    (74, 0x4E),  // KPMINUS
    (75, 0x56),  // KP4
    (76, 0x57),  // KP5
    (77, 0x58),  // KP6
    (78, 0x45),  // KPPLUS
    (79, 0x53),  // KP1
    (80, 0x54),  // KP2
    (81, 0x55),  // KP3
    (82, 0x52),  // KP0
    (83, 0x41),  // KPDOT        -> KeypadDecimal
    (87, 0x67),  // F11
    (88, 0x6F),  // F12
    (96, 0x4C),  // KPENTER
    (97, 0x3E),  // RIGHTCTRL
    (98, 0x4B),  // KPSLASH
    (100, 0x3D), // RIGHTALT     -> RightOption
    (102, 0x73), // HOME
    (103, 0x7E), // UP
    (104, 0x74), // PAGEUP
    (105, 0x7B), // LEFT
    (106, 0x7C), // RIGHT
    (107, 0x77), // END
    (108, 0x7D), // DOWN
    (109, 0x79), // PAGEDOWN
    (111, 0x75), // DELETE       -> ForwardDelete
    (113, 0x4A), // MUTE
    (114, 0x49), // VOLUMEDOWN
    (115, 0x48), // VOLUMEUP
    (117, 0x51), // KPEQUAL
    (119, 0x71), // PAUSE        -> F15, the closest mac analogue
    (125, 0x37), // LEFTMETA     -> Command
    (126, 0x36), // RIGHTMETA    -> RightCommand
    (183, 0x69), // F13
    (184, 0x6B), // F14
    (185, 0x71), // F15
    (186, 0x6A), // F16
    (187, 0x40), // F17
    (188, 0x4F), // F18
    (189, 0x50), // F19
    (190, 0x5A), // F20
];

/// Keys held as modifiers, which macOS reports through event flags rather than as
/// ordinary key events. Injection has to set the flag as well as post the key, or the
/// receiving app sees Shift go down but every subsequent character arrives lowercase.
pub(crate) const MODIFIER_EVDEV: &[u16] = &[
    29,  // LEFTCTRL
    42,  // LEFTSHIFT
    54,  // RIGHTSHIFT
    56,  // LEFTALT
    58,  // CAPSLOCK
    97,  // RIGHTCTRL
    100, // RIGHTALT
    125, // LEFTMETA
    126, // RIGHTMETA
];
