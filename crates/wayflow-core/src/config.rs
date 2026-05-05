// Screen layout configuration.
//
// Screens are arranged in a 2D grid. The server occupies (0,0). Client screens
// are placed relative to the server using cardinal edges.
//
// Example config (TOML):
//
//   [server]
//   name = "helicon"
//
//   [[clients]]
//   name    = "trantor"
//   edge    = "Right"    # trantor is to the right of helicon
//   offset  = 0          # vertical alignment offset in pixels

use serde::{Deserialize, Serialize};
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
