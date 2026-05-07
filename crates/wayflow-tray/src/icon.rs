//! Tray icons. To avoid shipping binary assets we render three solid-color
//! rounded squares procedurally at startup. Replace with real artwork later
//! by swapping `make_*` for `image::load_from_memory(include_bytes!(...))`.

use tray_icon::Icon;

pub fn idle() -> Icon {
    rounded_square(0x70, 0x70, 0x70, 0xFF) // gray
}

pub fn running() -> Icon {
    rounded_square(0x3B, 0xC4, 0x6E, 0xFF) // green
}

pub fn error() -> Icon {
    rounded_square(0xD0, 0x4A, 0x4A, 0xFF) // red
}

const SIZE: u32 = 32;
const CORNER: i32 = 6;

fn rounded_square(r: u8, g: u8, b: u8, a: u8) -> Icon {
    let w = SIZE as i32;
    let h = SIZE as i32;
    let mut buf = vec![0u8; (w * h * 4) as usize];

    for y in 0..h {
        for x in 0..w {
            let inside = if x < CORNER && y < CORNER {
                in_corner(x, y, CORNER)
            } else if x >= w - CORNER && y < CORNER {
                in_corner(w - 1 - x, y, CORNER)
            } else if x < CORNER && y >= h - CORNER {
                in_corner(x, h - 1 - y, CORNER)
            } else if x >= w - CORNER && y >= h - CORNER {
                in_corner(w - 1 - x, h - 1 - y, CORNER)
            } else {
                true
            };
            let idx = ((y * w + x) * 4) as usize;
            if inside {
                buf[idx] = r;
                buf[idx + 1] = g;
                buf[idx + 2] = b;
                buf[idx + 3] = a;
            }
        }
    }

    Icon::from_rgba(buf, SIZE, SIZE).expect("build tray icon")
}

fn in_corner(x: i32, y: i32, radius: i32) -> bool {
    let dx = radius - x;
    let dy = radius - y;
    dx * dx + dy * dy <= radius * radius
}
