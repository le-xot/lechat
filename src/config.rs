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
pub struct Appearance {
    #[serde(default = "default_font_size")]
    pub font_size: f32,
    /// `None` keeps the built-in font.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_family: Option<String>,
    #[serde(default = "default_true")]
    pub show_time: bool,
    #[serde(default)]
    pub show_seconds: bool,
    #[serde(default = "default_emote_scale")]
    pub emote_scale: f32,
}

fn default_emote_scale() -> f32 {
    1.8
}

fn default_true() -> bool {
    true
}

fn default_font_size() -> f32 {
    14.0
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            font_size: default_font_size(),
            font_family: None,
            show_time: true,
            show_seconds: false,
            emote_scale: default_emote_scale(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Updates {
    #[serde(default = "default_true")]
    pub check: bool,
}

impl Default for Updates {
    fn default() -> Self {
        Self { check: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Config {
    #[serde(default)]
    pub window: WindowCfg,
    #[serde(default)]
    pub appearance: Appearance,
    #[serde(default)]
    pub updates: Updates,
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
            appearance: Appearance::default(),
            updates: Updates::default(),
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

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("не удалось создать {}", parent.display()))?;
        }
        std::fs::write(path, self.to_toml())
            .with_context(|| format!("не удалось записать {}", path.display()))
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
    fn parses_and_saves_appearance() {
        let mut cfg = Config::parse(
            r#"
[[panel]]
kind = "twitch"
channel = "a"

[appearance]
font_size = 17.5
font_family = "DejaVu Sans"
show_time = false
show_seconds = true
"#,
        )
        .unwrap();
        assert_eq!(cfg.appearance.font_size, 17.5);
        assert_eq!(cfg.appearance.font_family.as_deref(), Some("DejaVu Sans"));
        assert!(!cfg.appearance.show_time);
        assert!(cfg.appearance.show_seconds);
        assert!(cfg.updates.check);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        cfg.appearance.font_family = None;
        cfg.save(&path).unwrap();
        let reread = Config::load_or_create(&path).unwrap();
        assert_eq!(reread.appearance.font_size, 17.5);
        assert!(reread.appearance.font_family.is_none());
    }

    #[test]
    fn appearance_defaults_when_absent() {
        let cfg = Config::parse(
            r#"
[[panel]]
kind = "twitch"
channel = "a"
"#,
        )
        .unwrap();
        assert_eq!(cfg.appearance.font_size, 14.0);
        assert!(cfg.appearance.font_family.is_none());
        assert!(cfg.appearance.show_time);
        assert!(!cfg.appearance.show_seconds);
        assert_eq!(cfg.appearance.emote_scale, 1.8);
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
