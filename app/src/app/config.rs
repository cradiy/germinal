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
    pub terminal: TerminalConfig,
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
            TerminalFontFamily::new(self.terminal.font_family.clone()),
            TerminalFontSize::new(self.terminal.font_size),
            size_config,
        )
    }
}

impl Default for GerminalConfig {
    fn default() -> Self {
        Self {
            window: WindowConfig::default(),
            terminal: TerminalConfig::default(),
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
pub struct TerminalConfig {
    pub font_family: String,
    pub font_size: f32,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            font_family: TerminalFontFamily::default().name().to_owned(),
            font_size: 16.0,
        }
    }
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

    use super::GerminalConfig;

    #[test]
    fn default_config_serializes_public_font_fields() {
        let contents = toml::to_string_pretty(&GerminalConfig::default()).unwrap();

        assert!(contents.contains(&format!(
            "font_family = {:?}",
            TerminalFontFamily::default().name()
        )));
        assert!(contents.contains("font_size = 16.0"));
    }

    #[test]
    fn terminal_profile_uses_configured_font() {
        let config: GerminalConfig = toml::from_str(
            r#"
            [terminal]
            font_family = "JetBrains Mono"
            font_size = 18.5
            "#,
        )
        .unwrap();
        let profile = config.terminal_profile();

        assert_eq!(profile.font_family().name(), "JetBrains Mono");
        assert_eq!(profile.font_size().logical_px(), 18.5);
    }
}
