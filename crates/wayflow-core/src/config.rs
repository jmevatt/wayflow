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
            let entry = ClientEntry { name: "test".into(), edge, offset: 0 };
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
            server: ServerConfig { name: "helicon".into(), port: 24800 },
            clients: vec![
                ClientEntry { name: "trantor".into(), edge: Edge::Right, offset: 0 },
                ClientEntry { name: "other".into(), edge: Edge::Bottom, offset: -50 },
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
            server: ServerConfig { name: "srv".into(), port: 9999 },
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
}
