use std::{
    env, fs,
    path::{Path, PathBuf},
};

use germinal_ports::pty_host::{
    font_family::TerminalFontFamily,
    font_size::TerminalFontSize,
    profile::TerminalProfile,
    size_info::{TerminalPadding, TerminalSizeConfig},
};
use serde::{Deserialize, Serialize};

use crate::app::error::{AppError, AppResult};

pub const APP_NAME: &str = "germinal";

const CONFIG_FILE_NAME: &str = "config.toml";

#[derive(Debug, Clone)]
pub struct AppPaths {
    config_dir: PathBuf,
    config_file: PathBuf,
    log_dir: PathBuf,
}

impl AppPaths {
    pub fn resolve() -> AppResult<Self> {
        let config_dir = xdg_dir("XDG_CONFIG_HOME", ".config")?.join(APP_NAME);
        let state_dir = xdg_dir("XDG_STATE_HOME", ".local/state")?.join(APP_NAME);
        let config_file = config_dir.join(CONFIG_FILE_NAME);
        let log_dir = state_dir.join("logs");

        Ok(Self {
            config_dir,
            config_file,
            log_dir,
        })
    }

    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    pub fn config_file(&self) -> &Path {
        &self.config_file
    }

    pub fn log_dir(&self) -> &Path {
        &self.log_dir
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GerminalConfig {
    pub window: WindowConfig,
    pub font: FontConfig,
    pub scrolling: ScrollingConfig,
    pub keyboard: KeyboardConfig,
    pub logging: LoggingConfig,
}

impl GerminalConfig {
    pub fn terminal_profile(&self) -> TerminalProfile {
        let default_profile = TerminalProfile::default();
        let size_config = TerminalSizeConfig::new(
            default_profile.size_config().cell_size(),
            TerminalPadding::ZERO,
        );

        TerminalProfile::new(
            TerminalFontFamily::new(self.font.normal.family.clone()),
            TerminalFontSize::new(self.font.size),
            size_config,
        )
    }
}

impl Default for GerminalConfig {
    fn default() -> Self {
        Self {
            window: WindowConfig::default(),
            font: FontConfig::default(),
            scrolling: ScrollingConfig::default(),
            keyboard: KeyboardConfig::default(),
            logging: LoggingConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowConfig {
    pub title: String,
    pub width_px: u32,
    pub height_px: u32,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            title: "Germinal".to_string(),
            width_px: 960,
            height_px: 540,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FontConfig {
    pub normal: FontFaceConfig,
    pub size: f32,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            normal: FontFaceConfig::default(),
            size: 16.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FontFaceConfig {
    pub family: String,
}

impl Default for FontFaceConfig {
    fn default() -> Self {
        Self {
            family: TerminalFontFamily::default().name().to_owned(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ScrollingConfig {
    pub history: usize,
}

impl Default for ScrollingConfig {
    fn default() -> Self {
        Self { history: 10_000 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct KeyboardConfig {
    pub bindings: Vec<KeyboardBinding>,
}

impl Default for KeyboardConfig {
    fn default() -> Self {
        Self {
            bindings: vec![KeyboardBinding {
                key: "Space".to_string(),
                mods: "Control|Shift".to_string(),
                action: KeyboardAction::ToggleViMode,
            }],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyboardBinding {
    pub key: String,
    #[serde(default)]
    pub mods: String,
    pub action: KeyboardAction,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum KeyboardAction {
    ToggleViMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    pub console_level: LogLevel,
    pub file_level: LogLevel,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            console_level: LogLevel::Debug,
            file_level: LogLevel::Info,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl Default for LogLevel {
    fn default() -> Self {
        Self::Info
    }
}

pub fn load_or_create_config() -> AppResult<(GerminalConfig, AppPaths)> {
    let paths = AppPaths::resolve()?;
    if !paths.config_file().exists() {
        write_default_config(&paths)?;
    }

    let contents =
        fs::read_to_string(paths.config_file()).map_err(|source| AppError::ReadConfig {
            path: paths.config_file().to_path_buf(),
            source,
        })?;
    let config = toml::from_str(&contents).map_err(|source| AppError::ParseConfig {
        path: paths.config_file().to_path_buf(),
        source,
    })?;

    Ok((config, paths))
}

fn write_default_config(paths: &AppPaths) -> AppResult<()> {
    create_dir_all(paths.config_dir())?;
    let contents =
        toml::to_string_pretty(&GerminalConfig::default()).map_err(AppError::SerializeConfig)?;
    fs::write(paths.config_file(), contents).map_err(|source| AppError::WriteConfig {
        path: paths.config_file().to_path_buf(),
        source,
    })
}

fn xdg_dir(env_name: &str, home_suffix: &str) -> AppResult<PathBuf> {
    match env::var_os(env_name) {
        Some(value) if !value.is_empty() => Ok(PathBuf::from(value)),
        _ => {
            let home = env::var_os("HOME").ok_or(AppError::MissingHomeDir)?;
            Ok(PathBuf::from(home).join(home_suffix))
        }
    }
}

pub fn create_dir_all(path: &Path) -> AppResult<()> {
    fs::create_dir_all(path).map_err(|source| AppError::CreateDirectory {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use germinal_ports::pty_host::font_family::TerminalFontFamily;

    use super::{GerminalConfig, KeyboardAction};

    #[test]
    fn default_config_serializes_alacritty_style_fields() {
        let contents = toml::to_string_pretty(&GerminalConfig::default()).unwrap();

        let value: toml::Value = toml::from_str(&contents).unwrap();
        assert_eq!(value["font"]["size"].as_float(), Some(16.0));
        assert_eq!(
            value["font"]["normal"]["family"].as_str(),
            Some(TerminalFontFamily::default().name())
        );
        assert_eq!(value["scrolling"]["history"].as_integer(), Some(10_000));
        assert_eq!(
            value["keyboard"]["bindings"][0]["action"].as_str(),
            Some("ToggleViMode")
        );
    }

    #[test]
    fn terminal_profile_uses_configured_font() {
        let config: GerminalConfig = toml::from_str(
            r#"
            font.normal = { family = "JetBrains Mono" }
            font.size = 18
            "#,
        )
        .unwrap();
        let profile = config.terminal_profile();

        assert_eq!(profile.font_family().name(), "JetBrains Mono");
        assert_eq!(profile.font_size().logical_px(), 18.0);
    }

    #[test]
    fn parses_scrollback_history_limit() {
        let config: GerminalConfig = toml::from_str(
            r#"
            [scrolling]
            history = 512
            "#,
        )
        .unwrap();

        assert_eq!(config.scrolling.history, 512);
        assert_eq!(config.keyboard.bindings.len(), 1);
        assert_eq!(
            config.keyboard.bindings[0].action,
            KeyboardAction::ToggleViMode
        );
    }

    #[test]
    fn parses_alacritty_style_keyboard_binding() {
        let config: GerminalConfig = toml::from_str(
            r#"
            [[keyboard.bindings]]
            key = "V"
            mods = "Control|Shift"
            action = "ToggleViMode"
            "#,
        )
        .unwrap();

        let binding = &config.keyboard.bindings[0];
        assert_eq!(binding.key, "V");
        assert_eq!(binding.mods, "Control|Shift");
        assert_eq!(binding.action, KeyboardAction::ToggleViMode);
    }
}
