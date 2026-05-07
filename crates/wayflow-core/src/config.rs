// Screen layout configuration.
//
// Server config (config.toml):
//   [server]
//   name = "helicon"
//
//   [[clients]]
//   name   = "trantor"
//   edge   = "Right"
//   offset = 0
//
// Client config (client.toml):
//   server = "helicon"
//   port   = 24800  # optional

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientEntry {
    /// Hostname or display name. Must match the name the client sends in HelloC2S.
    pub name: String,
    /// Which edge of the server screen leads to this client.
    pub edge: Edge,
    /// Pixel offset along the perpendicular axis (e.g. vertical offset for Left/Right edges).
    #[serde(default)]
    pub offset: i32,
    /// Modifier key remap applied when forwarding keyboard events to this client.
    /// Keys and values are modifier names: ctrl_left, ctrl_right, shift_left, shift_right,
    /// alt, alt_gr, meta_left, meta_right.
    /// Example: { ctrl_left = "meta_left", meta_left = "ctrl_left" }
    #[serde(default)]
    pub modifier_map: HashMap<String, String>,
}

/// Map a modifier key name to its USB HID usage code (page 0x07).
///
/// Canonical names match the HID modifier layout (`<key>_left` / `<key>_right`).
/// A bunch of aliases are accepted so `modifier_map` reads naturally regardless
/// of what people call these keys on their physical keyboard:
///   - meta/super/cmd/command/win/windows -> 0xE3 (left) / 0xE7 (right)
///   - alt/option/opt                      -> 0xE2 (left) / 0xE6 (right)
///   - alt_gr is a synonym for alt_right.
pub fn modifier_name_to_hid(name: &str) -> Option<u32> {
    match name {
        // Ctrl
        "ctrl_left" | "control_left" => Some(0xE0),
        "ctrl_right" | "control_right" => Some(0xE4),

        // Shift
        "shift_left" => Some(0xE1),
        "shift_right" => Some(0xE5),

        // Alt / Option
        "alt" | "alt_left" | "option" | "option_left" | "opt" | "opt_left" => Some(0xE2),
        "alt_gr" | "alt_right" | "option_right" | "opt_right" => Some(0xE6),

        // Meta / Super / Cmd / Windows
        "meta_left" | "super" | "super_left" | "cmd" | "cmd_left" | "command" | "command_left"
        | "win" | "win_left" | "windows" | "windows_left" => Some(0xE3),
        "meta_right" | "super_right" | "cmd_right" | "command_right" | "win_right"
        | "windows_right" => Some(0xE7),

        _ => None,
    }
}

/// Apply a client's modifier_map to a HID keycode.
/// Returns the remapped code, or the original if it has no mapping.
pub fn remap_modifier_key(map: &HashMap<String, String>, hid: u32) -> u32 {
    for (from_name, to_name) in map {
        if modifier_name_to_hid(from_name) == Some(hid) {
            if let Some(to) = modifier_name_to_hid(to_name) {
                return to;
            }
        }
    }
    hid
}

/// Walk a `modifier_map` and `tracing::warn!` for any name (key or value) that
/// `modifier_name_to_hid` does not recognise. Unknown names are silently
/// dropped at remap time, so the only way to notice a typo without this is to
/// stare at debug-level logs while pressing keys. Call from config load + every
/// hot-reload so a bad map surfaces immediately.
pub fn warn_unknown_modifier_names(client_name: &str, map: &HashMap<String, String>) {
    for (from, to) in map {
        if modifier_name_to_hid(from).is_none() {
            tracing::warn!(
                "client {client_name:?}: modifier_map key {from:?} is not a recognised \
                 modifier name -- this entry will be ignored. \
                 Accepted: ctrl_left/right, shift_left/right, alt_left/right (a.k.a. option, \
                 alt_gr), meta_left/right (a.k.a. super, cmd, command, windows)."
            );
        }
        if modifier_name_to_hid(to).is_none() {
            tracing::warn!(
                "client {client_name:?}: modifier_map value {to:?} (for key {from:?}) is not a \
                 recognised modifier name -- the {from:?} key will pass through unmapped."
            );
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub name: String,
    /// TCP port to listen on. Default 24800.
    #[serde(default = "default_port")]
    pub port: u16,
}

fn default_port() -> u16 {
    24800
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    #[serde(default)]
    pub clients: Vec<ClientEntry>,
}

impl Config {
    pub fn default_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("wayflow")
            .join("config.toml")
    }

    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Ok(toml::from_str(&text)?)
    }

    pub fn save(&self, path: &std::path::Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }
}

/// Minimal client-side config: just the server address.
/// Stored separately from the server config so clients don't need placement info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientConfig {
    /// Server hostname or IP address.
    pub server: String,
    /// Server port. Default 24800.
    #[serde(default = "default_port")]
    pub port: u16,
}

impl ClientConfig {
    pub fn default_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("wayflow")
            .join("client.toml")
    }

    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Ok(toml::from_str(&text)?)
    }

    pub fn save(&self, path: &std::path::Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn server_addr(&self) -> String {
        format!("{}:{}", self.server, self.port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn default_path_contains_wayflow() {
        let p = Config::default_path();
        assert!(p.to_string_lossy().contains("wayflow"));
    }

    #[test]
    fn default_port_is_24800() {
        assert_eq!(default_port(), 24800);
    }

    #[test]
    fn all_edge_variants_roundtrip_in_client_entry() {
        // TOML can't serialize a bare enum at top level; test via ClientEntry.
        for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
            let entry = ClientEntry {
                name: "test".into(),
                edge,
                offset: 0,
                modifier_map: Default::default(),
            };
            let s = toml::to_string(&entry).unwrap();
            let back: ClientEntry = toml::from_str(&s).unwrap();
            assert_eq!(back.edge, edge, "failed for {edge:?}");
        }
    }

    #[test]
    fn config_save_and_load_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let original = Config {
            server: ServerConfig {
                name: "helicon".into(),
                port: 24800,
            },
            clients: vec![
                ClientEntry {
                    name: "trantor".into(),
                    edge: Edge::Right,
                    offset: 0,
                    modifier_map: Default::default(),
                },
                ClientEntry {
                    name: "other".into(),
                    edge: Edge::Bottom,
                    offset: -50,
                    modifier_map: Default::default(),
                },
            ],
        };
        original.save(&path).unwrap();

        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.server.name, "helicon");
        assert_eq!(loaded.server.port, 24800);
        assert_eq!(loaded.clients.len(), 2);
        assert_eq!(loaded.clients[0].name, "trantor");
        assert_eq!(loaded.clients[0].edge, Edge::Right);
        assert_eq!(loaded.clients[0].offset, 0);
        assert_eq!(loaded.clients[1].name, "other");
        assert_eq!(loaded.clients[1].edge, Edge::Bottom);
        assert_eq!(loaded.clients[1].offset, -50);
    }

    #[test]
    fn config_load_minimal_toml() {
        let toml = r#"
[server]
name = "minimal"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.server.name, "minimal");
        assert_eq!(config.server.port, 24800); // default
        assert!(config.clients.is_empty()); // default
    }

    #[test]
    fn config_save_creates_parent_dirs() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("deep").join("nested").join("config.toml");

        let config = Config {
            server: ServerConfig {
                name: "srv".into(),
                port: 9999,
            },
            clients: vec![],
        };
        config.save(&path).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn config_load_nonexistent_file_errors() {
        let result = Config::load(std::path::Path::new("/nonexistent/path/config.toml"));
        assert!(result.is_err());
    }

    #[test]
    fn modifier_name_to_hid_known() {
        assert_eq!(modifier_name_to_hid("ctrl_left"), Some(0xE0));
        assert_eq!(modifier_name_to_hid("shift_left"), Some(0xE1));
        assert_eq!(modifier_name_to_hid("alt"), Some(0xE2));
        assert_eq!(modifier_name_to_hid("meta_left"), Some(0xE3));
        assert_eq!(modifier_name_to_hid("ctrl_right"), Some(0xE4));
        assert_eq!(modifier_name_to_hid("meta_right"), Some(0xE7));
        assert_eq!(modifier_name_to_hid("bogus"), None);
    }

    #[test]
    fn modifier_name_aliases() {
        // Left alt aliases all resolve to 0xE2
        for n in [
            "alt",
            "alt_left",
            "option",
            "option_left",
            "opt",
            "opt_left",
        ] {
            assert_eq!(
                modifier_name_to_hid(n),
                Some(0xE2),
                "alias {n} should be 0xE2"
            );
        }
        // Right alt aliases all resolve to 0xE6
        for n in ["alt_gr", "alt_right", "option_right", "opt_right"] {
            assert_eq!(
                modifier_name_to_hid(n),
                Some(0xE6),
                "alias {n} should be 0xE6"
            );
        }
        // Left meta aliases (super, cmd, command, win, windows) all resolve to 0xE3
        for n in [
            "meta_left",
            "super",
            "super_left",
            "cmd",
            "cmd_left",
            "command",
            "command_left",
            "win",
            "win_left",
            "windows",
            "windows_left",
        ] {
            assert_eq!(
                modifier_name_to_hid(n),
                Some(0xE3),
                "alias {n} should be 0xE3"
            );
        }
        // Right meta aliases all resolve to 0xE7
        for n in [
            "meta_right",
            "super_right",
            "cmd_right",
            "command_right",
            "win_right",
            "windows_right",
        ] {
            assert_eq!(
                modifier_name_to_hid(n),
                Some(0xE7),
                "alias {n} should be 0xE7"
            );
        }
        // Control aliases
        assert_eq!(modifier_name_to_hid("control_left"), Some(0xE0));
        assert_eq!(modifier_name_to_hid("control_right"), Some(0xE4));
    }

    #[test]
    fn remap_swap_alt_meta_using_left_right_aliases() {
        // The exact map shape Jordan was using -- previously a silent no-op
        // because alt_left / alt_right weren't recognised.
        let map: HashMap<String, String> = [
            ("alt_left".into(), "meta_left".into()),
            ("alt_right".into(), "meta_right".into()),
            ("meta_left".into(), "alt_left".into()),
            ("meta_right".into(), "alt_right".into()),
        ]
        .into();

        assert_eq!(remap_modifier_key(&map, 0xE2), 0xE3); // alt_left -> meta_left
        assert_eq!(remap_modifier_key(&map, 0xE3), 0xE2); // meta_left -> alt_left
        assert_eq!(remap_modifier_key(&map, 0xE6), 0xE7); // alt_right -> meta_right
        assert_eq!(remap_modifier_key(&map, 0xE7), 0xE6); // meta_right -> alt_right
        assert_eq!(remap_modifier_key(&map, 0xE0), 0xE0); // ctrl unchanged
    }

    #[test]
    fn remap_modifier_key_swaps_ctrl_meta() {
        let map: HashMap<String, String> = [
            ("ctrl_left".into(), "meta_left".into()),
            ("meta_left".into(), "ctrl_left".into()),
            ("ctrl_right".into(), "meta_right".into()),
            ("meta_right".into(), "ctrl_right".into()),
        ]
        .into();

        assert_eq!(remap_modifier_key(&map, 0xE0), 0xE3); // ctrl_left -> meta_left
        assert_eq!(remap_modifier_key(&map, 0xE3), 0xE0); // meta_left -> ctrl_left
        assert_eq!(remap_modifier_key(&map, 0xE4), 0xE7); // ctrl_right -> meta_right
        assert_eq!(remap_modifier_key(&map, 0xE7), 0xE4); // meta_right -> ctrl_right
        assert_eq!(remap_modifier_key(&map, 0xE1), 0xE1); // shift_left unchanged
        assert_eq!(remap_modifier_key(&map, 0x04), 0x04); // key A unchanged
    }

    #[test]
    fn remap_modifier_key_empty_map_passthrough() {
        let map: HashMap<String, String> = HashMap::new();
        assert_eq!(remap_modifier_key(&map, 0xE0), 0xE0);
        assert_eq!(remap_modifier_key(&map, 0x04), 0x04);
    }

    #[test]
    fn modifier_map_parses_from_toml() {
        let toml = r#"
name = "macbook"
edge = "Right"
modifier_map = { ctrl_left = "meta_left", meta_left = "ctrl_left" }
"#;
        let entry: ClientEntry = toml::from_str(toml).unwrap();
        assert_eq!(remap_modifier_key(&entry.modifier_map, 0xE0), 0xE3);
        assert_eq!(remap_modifier_key(&entry.modifier_map, 0xE3), 0xE0);
    }
}
