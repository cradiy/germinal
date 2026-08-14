use std::{
    env, fs,
    path::{Path, PathBuf},
};

use germinal_ports::pty_host::{
    cursor_style::{TerminalCursorShape, TerminalCursorStyle},
    font_family::TerminalFontFamily,
    font_size::TerminalFontSize,
    profile::TerminalProfile,
    size_info::{TerminalPadding, TerminalSizeConfig},
};
use germinal_ports::rendering::tab_bar::TabBarPosition;
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
    pub cursor: CursorConfig,
    pub scrolling: ScrollingConfig,
    pub bell: BellConfig,
    pub tabs: TabsConfig,
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

    pub fn terminal_cursor_style(&self) -> TerminalCursorStyle {
        TerminalCursorStyle::new(self.cursor.shape.into(), self.cursor.blinking)
    }
}

impl Default for GerminalConfig {
    fn default() -> Self {
        Self {
            window: WindowConfig::default(),
            font: FontConfig::default(),
            cursor: CursorConfig::default(),
            scrolling: ScrollingConfig::default(),
            bell: BellConfig::default(),
            tabs: TabsConfig::default(),
            keyboard: KeyboardConfig::default(),
            logging: LoggingConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CursorConfig {
    pub shape: CursorShape,
    pub blinking: bool,
    pub blink_interval_ms: u64,
}

impl Default for CursorConfig {
    fn default() -> Self {
        Self {
            shape: CursorShape::Block,
            blinking: false,
            blink_interval_ms: 750,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CursorShape {
    #[default]
    Block,
    Underline,
    Beam,
}

impl From<CursorShape> for TerminalCursorShape {
    fn from(shape: CursorShape) -> Self {
        match shape {
            CursorShape::Block => Self::Block,
            CursorShape::Underline => Self::Underline,
            CursorShape::Beam => Self::Beam,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BellConfig {
    pub duration_ms: u64,
    pub urgent_on_unfocused: bool,
}

impl Default for BellConfig {
    fn default() -> Self {
        Self {
            duration_ms: 150,
            urgent_on_unfocused: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TabsConfig {
    pub position: TabBarPosition,
}

impl Default for TabsConfig {
    fn default() -> Self {
        Self {
            position: TabBarPosition::Bottom,
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
            bindings: vec![
                KeyboardBinding {
                    key: "Space".to_string(),
                    mods: "Control|Shift".to_string(),
                    action: KeyboardAction::ToggleViMode,
                },
                KeyboardBinding {
                    key: "D".to_string(),
                    mods: "Control|Shift".to_string(),
                    action: KeyboardAction::SplitHorizontal,
                },
                KeyboardBinding {
                    key: "D".to_string(),
                    mods: "Control|Shift|Alt".to_string(),
                    action: KeyboardAction::SplitVertical,
                },
                KeyboardBinding {
                    key: "T".to_string(),
                    mods: "Control|Shift".to_string(),
                    action: KeyboardAction::NewTab,
                },
                KeyboardBinding {
                    key: "Left".to_string(),
                    mods: "Control|Shift".to_string(),
                    action: KeyboardAction::PreviousTab,
                },
                KeyboardBinding {
                    key: "Right".to_string(),
                    mods: "Control|Shift".to_string(),
                    action: KeyboardAction::NextTab,
                },
                KeyboardBinding {
                    key: "H".to_string(),
                    mods: "Control|Shift".to_string(),
                    action: KeyboardAction::PreviousTab,
                },
                KeyboardBinding {
                    key: "L".to_string(),
                    mods: "Control|Shift".to_string(),
                    action: KeyboardAction::NextTab,
                },
                KeyboardBinding {
                    key: "W".to_string(),
                    mods: "Control|Shift".to_string(),
                    action: KeyboardAction::ClosePane,
                },
                KeyboardBinding {
                    key: "Left".to_string(),
                    mods: "Control|Alt".to_string(),
                    action: KeyboardAction::FocusPaneLeft,
                },
                KeyboardBinding {
                    key: "Right".to_string(),
                    mods: "Control|Alt".to_string(),
                    action: KeyboardAction::FocusPaneRight,
                },
                KeyboardBinding {
                    key: "Up".to_string(),
                    mods: "Control|Alt".to_string(),
                    action: KeyboardAction::FocusPaneUp,
                },
                KeyboardBinding {
                    key: "Down".to_string(),
                    mods: "Control|Alt".to_string(),
                    action: KeyboardAction::FocusPaneDown,
                },
                KeyboardBinding {
                    key: "Left".to_string(),
                    mods: "Control|Shift|Alt".to_string(),
                    action: KeyboardAction::SwapPaneLeft,
                },
                KeyboardBinding {
                    key: "Right".to_string(),
                    mods: "Control|Shift|Alt".to_string(),
                    action: KeyboardAction::SwapPaneRight,
                },
                KeyboardBinding {
                    key: "Up".to_string(),
                    mods: "Control|Shift|Alt".to_string(),
                    action: KeyboardAction::SwapPaneUp,
                },
                KeyboardBinding {
                    key: "Down".to_string(),
                    mods: "Control|Shift|Alt".to_string(),
                    action: KeyboardAction::SwapPaneDown,
                },
            ],
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
    NewTab,
    NextTab,
    PreviousTab,
    SplitHorizontal,
    SplitVertical,
    FocusNextPane,
    FocusPreviousPane,
    FocusPaneLeft,
    FocusPaneRight,
    FocusPaneUp,
    FocusPaneDown,
    ClosePane,
    SwapPaneLeft,
    SwapPaneRight,
    SwapPaneUp,
    SwapPaneDown,
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
    use germinal_ports::pty_host::{
        cursor_style::{TerminalCursorShape, TerminalCursorStyle},
        font_family::TerminalFontFamily,
    };
    use germinal_ports::rendering::tab_bar::TabBarPosition;

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
        assert_eq!(value["cursor"]["shape"].as_str(), Some("block"));
        assert_eq!(value["cursor"]["blinking"].as_bool(), Some(false));
        assert_eq!(value["cursor"]["blink_interval_ms"].as_integer(), Some(750));
        assert_eq!(value["bell"]["duration_ms"].as_integer(), Some(150));
        assert_eq!(value["bell"]["urgent_on_unfocused"].as_bool(), Some(true));
        assert_eq!(value["tabs"]["position"].as_str(), Some("bottom"));
        assert_eq!(
            value["keyboard"]["bindings"][0]["action"].as_str(),
            Some("ToggleViMode")
        );
    }

    #[test]
    fn parses_cursor_configuration() {
        let config: GerminalConfig = toml::from_str(
            r#"
            [cursor]
            shape = "beam"
            blinking = true
            blink_interval_ms = 320
            "#,
        )
        .unwrap();

        assert_eq!(
            config.terminal_cursor_style(),
            TerminalCursorStyle::new(TerminalCursorShape::Beam, true)
        );
        assert_eq!(config.cursor.blink_interval_ms, 320);
    }

    #[test]
    fn parses_top_tab_bar_position() {
        let config: GerminalConfig = toml::from_str(
            r#"
            [tabs]
            position = "top"
            "#,
        )
        .unwrap();

        assert_eq!(config.tabs.position, TabBarPosition::Top);
    }

    #[test]
    fn parses_visual_bell_configuration() {
        let config: GerminalConfig = toml::from_str(
            r#"
            [bell]
            duration_ms = 240
            urgent_on_unfocused = false
            "#,
        )
        .unwrap();

        assert_eq!(config.bell.duration_ms, 240);
        assert!(!config.bell.urgent_on_unfocused);
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
        assert_eq!(config.keyboard.bindings.len(), 17);
        assert_eq!(
            config.keyboard.bindings[0].action,
            KeyboardAction::ToggleViMode
        );
        assert_eq!(
            config.keyboard.bindings[1].action,
            KeyboardAction::SplitHorizontal
        );
        assert_eq!(config.keyboard.bindings[1].key, "D");
        assert_eq!(config.keyboard.bindings[1].mods, "Control|Shift");
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

    #[test]
    fn parses_workspace_keyboard_actions() {
        let config: GerminalConfig = toml::from_str(
            r#"
            [[keyboard.bindings]]
            key = "D"
            mods = "Control|Shift"
            action = "SplitHorizontal"

            [[keyboard.bindings]]
            key = "W"
            mods = "Control|Shift"
            action = "ClosePane"
            "#,
        )
        .unwrap();

        assert_eq!(config.keyboard.bindings.len(), 2);
        assert_eq!(
            config.keyboard.bindings[0].action,
            KeyboardAction::SplitHorizontal
        );
        assert_eq!(
            config.keyboard.bindings[1].action,
            KeyboardAction::ClosePane
        );
    }
}
