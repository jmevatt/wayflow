//! Wayflow tray app: a tiny menu-bar/system-tray UI that supervises a
//! `wayflow server` or `wayflow client` child process and exposes a small
//! egui settings window.
//!
//! Architecture:
//!   - The tray is a separate subcommand of the wayflow binary (`wayflow tray`).
//!   - Server/client modes run as child processes spawned via current_exe().
//!     If the child crashes the tray stays alive and shows the error state.
//!   - Settings window is an egui panel hosted in eframe; it edits
//!     config.toml / client.toml on disk and the supervisor picks up the
//!     new values on the next start.

pub mod app;
pub mod supervisor;
pub mod icon;

pub use app::run;
