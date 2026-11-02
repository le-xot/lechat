use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PanelKind {
    Twitch,
    Streamx,
}

impl PanelKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Twitch => "Twitch",
            Self::Streamx => "streamx",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WindowCfg {
    #[serde(default = "default_width")]
    pub width: f32,
    #[serde(default = "default_height")]
    pub height: f32,
}

fn default_width() -> f32 {
    1100.0
}

fn default_height() -> f32 {
    760.0
}

impl Default for WindowCfg {
    fn default() -> Self {
        Self {
            width: default_width(),
            height: default_height(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PanelCfg {
    pub kind: PanelKind,
    pub channel: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

impl PanelCfg {
    pub fn streamx_base_url(&self) -> &str {
        self.base_url
            .as_deref()
            .unwrap_or("https://streamx.satont.dev")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Config {
    #[serde(default)]
    pub window: WindowCfg,
    #[serde(default)]
    pub panel: Vec<PanelCfg>,
}

impl Config {
    pub fn parse(text: &str) -> Result<Config> {
        let cfg: Config = toml::from_str(text).context("не удалось разобрать конфиг")?;
        if cfg.panel.is_empty() {
            anyhow::bail!("в конфиге нет ни одного канала ([panel])");
        }
        for panel in &cfg.panel {
            if panel.channel.trim().is_empty() {
                anyhow::bail!("у канала пустое имя");
            }
        }
        Ok(cfg)
    }

    pub fn load_or_create(path: &Path) -> Result<Config> {
        if path.exists() {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("не удалось прочитать {}", path.display()))?;
            return Self::parse(&text);
        }
        let cfg = Self::default_config();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("не удалось создать {}", parent.display()))?;
        }
        std::fs::write(path, cfg.to_toml())
            .with_context(|| format!("не удалось записать {}", path.display()))?;
        Ok(cfg)
    }

    pub fn default_config() -> Config {
        Config {
            window: WindowCfg::default(),
            panel: vec![
                PanelCfg {
                    kind: PanelKind::Twitch,
                    channel: "etonelexot".into(),
                    base_url: None,
                },
                PanelCfg {
                    kind: PanelKind::Streamx,
                    channel: "le-xot".into(),
                    base_url: None,
                },
            ],
        }
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }

    pub fn default_path() -> PathBuf {
        directories::ProjectDirs::from("dev", "le-xot", "lechat")
            .map(|dirs| dirs.config_dir().join("config.toml"))
            .unwrap_or_else(|| PathBuf::from("lechat.toml"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_two_panels() {
        let cfg = Config::parse(
            r#"
[window]
width = 900
height = 700

[[panel]]
kind = "twitch"
channel = "etonelexot"

[[panel]]
kind = "streamx"
channel = "le-xot"
base_url = "https://streamx.satont.dev"
"#,
        )
        .unwrap();
        assert_eq!(cfg.panel.len(), 2);
        assert_eq!(cfg.panel[0].kind, PanelKind::Twitch);
        assert_eq!(
            cfg.panel[1].base_url.as_deref(),
            Some("https://streamx.satont.dev")
        );
    }

    #[test]
    fn rejects_empty_panels() {
        assert!(Config::parse("[window]\nwidth = 900\nheight = 700\n").is_err());
    }

    #[test]
    fn writes_default_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let cfg = Config::load_or_create(&path).unwrap();
        assert_eq!(cfg.panel.len(), 2);
        assert_eq!(cfg.panel[0].channel, "etonelexot");
        assert_eq!(cfg.panel[1].channel, "le-xot");
        assert!(path.exists());
    }
}
