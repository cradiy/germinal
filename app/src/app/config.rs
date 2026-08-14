use std::{
    collections::BTreeMap,
    env, fs,
    path::{Component, Path, PathBuf},
};

use germinal_ports::pty_host::{
    color_theme::TerminalColorTheme,
    cursor_style::{TerminalCursorShape, TerminalCursorStyle},
    font_config::TerminalFontConfig,
    font_face::TerminalFontFace,
    font_family::TerminalFontFamily,
    font_size::TerminalFontSize,
    profile::TerminalProfile,
    size_info::{TerminalPadding, TerminalSizeConfig},
    spawn_config::PtyShellCommand,
    terminal_clipboard::TerminalOsc52Mode,
};
use germinal_ports::rendering::tab_bar::TabBarPosition;
use serde::{Deserialize, Serialize};

use crate::app::error::{AppError, AppResult};

mod kitty_colors;

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
    pub colors: ColorsConfig,
    pub terminal: TerminalConfig,
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

        let normal = terminal_font_face(&self.font.normal);
        let bold = self.font.bold.as_ref().map(terminal_font_face);
        let italic = self.font.italic.as_ref().map(terminal_font_face);
        let bold_italic = self.font.bold_italic.as_ref().map(terminal_font_face);
        let fallbacks = self
            .font
            .fallback
            .iter()
            .cloned()
            .map(TerminalFontFamily::new)
            .collect();
        let font_config = TerminalFontConfig::new(
            normal.family().clone(),
            TerminalFontSize::new(self.font.size),
        )
        .with_faces(normal, bold, italic, bold_italic, fallbacks);

        TerminalProfile::new(
            font_config.family().clone(),
            font_config.size(),
            size_config,
        )
        .with_font_config(font_config)
    }

    pub fn terminal_cursor_style(&self) -> TerminalCursorStyle {
        TerminalCursorStyle::new(self.cursor.shape.into(), self.cursor.blinking)
    }

    pub fn terminal_osc52_mode(&self) -> TerminalOsc52Mode {
        self.terminal.osc52.into()
    }

    pub fn terminal_color_theme(&self) -> TerminalColorTheme {
        self.colors.resolved
    }

    pub fn pty_shell_command(&self) -> Option<PtyShellCommand> {
        self.terminal.shell.as_ref().map(|shell| {
            let program = expand_home(Path::new(&shell.program))
                .to_string_lossy()
                .into_owned();
            PtyShellCommand::new(program, shell.args.clone())
        })
    }

    pub fn configured_working_directory(&self) -> Option<PathBuf> {
        self.terminal.working_directory.as_deref().map(expand_home)
    }

    fn validate(&self) -> Result<(), String> {
        if !self.window.opacity.is_finite() || !(0.0..=1.0).contains(&self.window.opacity) {
            return Err("window.opacity must be a finite number between 0.0 and 1.0".to_string());
        }

        for (name, face) in [
            ("font.normal", Some(&self.font.normal)),
            ("font.bold", self.font.bold.as_ref()),
            ("font.italic", self.font.italic.as_ref()),
            ("font.bold_italic", self.font.bold_italic.as_ref()),
        ] {
            if let Some(face) = face {
                if face.family.trim().is_empty() {
                    return Err(format!("{name}.family must not be empty"));
                }
                if face
                    .style
                    .as_deref()
                    .is_some_and(|style| style.trim().is_empty())
                {
                    return Err(format!("{name}.style must not be empty"));
                }
            }
        }
        if self
            .font
            .fallback
            .iter()
            .any(|family| family.trim().is_empty())
        {
            return Err("font.fallback entries must not be empty".to_string());
        }

        if let Some(working_directory) = self.configured_working_directory()
            && !working_directory.is_dir()
        {
            return Err(format!(
                "working_directory is not an existing directory: {}",
                working_directory.display()
            ));
        }

        if let Some(shell) = self.pty_shell_command() {
            if shell.program.trim().is_empty() {
                return Err("terminal.shell.program must not be empty".to_string());
            }
            if !program_exists(&shell.program) {
                return Err(format!(
                    "terminal.shell.program was not found or is not executable: {}",
                    shell.program
                ));
            }
        }

        Ok(())
    }
}

fn terminal_font_face(config: &FontFaceConfig) -> TerminalFontFace {
    let face = TerminalFontFace::new(TerminalFontFamily::new(config.family.clone()));
    match config.style.as_deref() {
        Some(style) => face.with_style(style),
        None => face,
    }
}

fn expand_home(path: &Path) -> PathBuf {
    let mut components = path.components();
    if components.next() != Some(Component::Normal("~".as_ref())) {
        return path.to_path_buf();
    }

    let Some(home) = env::var_os("HOME") else {
        return path.to_path_buf();
    };
    PathBuf::from(home).join(components.as_path())
}

impl Default for GerminalConfig {
    fn default() -> Self {
        Self {
            window: WindowConfig::default(),
            font: FontConfig::default(),
            cursor: CursorConfig::default(),
            colors: ColorsConfig::default(),
            terminal: TerminalConfig::default(),
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
pub struct ColorsConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<PathBuf>,
    #[serde(flatten)]
    pub overrides: BTreeMap<String, String>,
    #[serde(skip)]
    resolved: TerminalColorTheme,
}

impl ColorsConfig {
    fn resolve(&mut self, config_dir: &Path) -> Result<(), String> {
        self.resolved = kitty_colors::resolve_color_theme(self, config_dir)?;
        Ok(())
    }
}

impl Default for ColorsConfig {
    fn default() -> Self {
        Self {
            theme: None,
            overrides: BTreeMap::new(),
            resolved: TerminalColorTheme::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalConfig {
    pub osc52: Osc52Policy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell: Option<ShellConfig>,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            osc52: Osc52Policy::OnlyCopy,
            working_directory: None,
            shell: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShellConfig {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum Osc52Policy {
    Disabled,
    #[default]
    OnlyCopy,
    OnlyPaste,
    CopyPaste,
}

impl From<Osc52Policy> for TerminalOsc52Mode {
    fn from(policy: Osc52Policy) -> Self {
        match policy {
            Osc52Policy::Disabled => Self::Disabled,
            Osc52Policy::OnlyCopy => Self::OnlyCopy,
            Osc52Policy::OnlyPaste => Self::OnlyPaste,
            Osc52Policy::CopyPaste => Self::CopyPaste,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<BellCommand>,
}

impl Default for BellConfig {
    fn default() -> Self {
        Self {
            duration_ms: 150,
            urgent_on_unfocused: true,
            command: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BellCommand {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
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
    pub opacity: f32,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            title: "Germinal".to_string(),
            width_px: 960,
            height_px: 540,
            opacity: 1.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FontConfig {
    pub normal: FontFaceConfig,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bold: Option<FontFaceConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub italic: Option<FontFaceConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bold_italic: Option<FontFaceConfig>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fallback: Vec<String>,
    pub size: f32,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            normal: FontFaceConfig::default(),
            bold: None,
            italic: None,
            bold_italic: None,
            fallback: Vec::new(),
            size: 16.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FontFaceConfig {
    pub family: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
}

impl Default for FontFaceConfig {
    fn default() -> Self {
        Self {
            family: TerminalFontFamily::default().name().to_owned(),
            style: None,
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
                    key: "F".to_string(),
                    mods: "Control|Shift".to_string(),
                    action: KeyboardAction::ToggleSearch,
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
                    key: "H".to_string(),
                    mods: "Control|Shift|Alt".to_string(),
                    action: KeyboardAction::MoveTabLeft,
                },
                KeyboardBinding {
                    key: "L".to_string(),
                    mods: "Control|Shift|Alt".to_string(),
                    action: KeyboardAction::MoveTabRight,
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
                KeyboardBinding {
                    key: "Left".to_string(),
                    mods: "Alt|Shift".to_string(),
                    action: KeyboardAction::ResizePaneLeft,
                },
                KeyboardBinding {
                    key: "Right".to_string(),
                    mods: "Alt|Shift".to_string(),
                    action: KeyboardAction::ResizePaneRight,
                },
                KeyboardBinding {
                    key: "Up".to_string(),
                    mods: "Alt|Shift".to_string(),
                    action: KeyboardAction::ResizePaneUp,
                },
                KeyboardBinding {
                    key: "Down".to_string(),
                    mods: "Alt|Shift".to_string(),
                    action: KeyboardAction::ResizePaneDown,
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
    ToggleSearch,
    NewTab,
    NextTab,
    PreviousTab,
    MoveTabLeft,
    MoveTabRight,
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
    ResizePaneLeft,
    ResizePaneRight,
    ResizePaneUp,
    ResizePaneDown,
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
    let mut config: GerminalConfig =
        toml::from_str(&contents).map_err(|source| AppError::ParseConfig {
            path: paths.config_file().to_path_buf(),
            source,
        })?;
    config
        .validate()
        .map_err(|message| AppError::InvalidConfig {
            path: paths.config_file().to_path_buf(),
            message,
        })?;
    config
        .colors
        .resolve(paths.config_dir())
        .map_err(|message| AppError::InvalidConfig {
            path: paths.config_file().to_path_buf(),
            message,
        })?;

    Ok((config, paths))
}

fn program_exists(program: &str) -> bool {
    let path = Path::new(program);
    if path.is_absolute() || path.components().count() > 1 {
        return is_executable_file(path);
    }

    env::split_paths(&env::var_os("PATH").unwrap_or_default())
        .map(|directory| directory.join(program))
        .any(|candidate| is_executable_file(&candidate))
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }

    #[cfg(not(unix))]
    {
        true
    }
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
        terminal_clipboard::TerminalOsc52Mode,
    };
    use germinal_ports::rendering::tab_bar::TabBarPosition;

    use super::{GerminalConfig, KeyboardAction, ShellConfig};

    #[test]
    fn default_config_serializes_alacritty_style_fields() {
        let contents = toml::to_string_pretty(&GerminalConfig::default()).unwrap();

        let value: toml::Value = toml::from_str(&contents).unwrap();
        assert_eq!(value["window"]["opacity"].as_float(), Some(1.0));
        assert_eq!(value["font"]["size"].as_float(), Some(16.0));
        assert_eq!(
            value["font"]["normal"]["family"].as_str(),
            Some(TerminalFontFamily::default().name())
        );
        assert_eq!(value["scrolling"]["history"].as_integer(), Some(10_000));
        assert_eq!(value["cursor"]["shape"].as_str(), Some("block"));
        assert_eq!(value["cursor"]["blinking"].as_bool(), Some(false));
        assert_eq!(value["cursor"]["blink_interval_ms"].as_integer(), Some(750));
        assert_eq!(value["terminal"]["osc52"].as_str(), Some("OnlyCopy"));
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
    fn validates_window_opacity() {
        let config: GerminalConfig = toml::from_str(
            r#"
            [window]
            opacity = 0.82
            "#,
        )
        .unwrap();
        assert_eq!(config.window.opacity, 0.82);
        assert!(config.validate().is_ok());

        for opacity in [-0.1, 1.1, f32::INFINITY, f32::NAN] {
            let mut config = GerminalConfig::default();
            config.window.opacity = opacity;
            assert!(config.validate().unwrap_err().contains("window.opacity"));
        }
    }

    #[test]
    fn parses_visual_bell_configuration() {
        let config: GerminalConfig = toml::from_str(
            r#"
            [bell]
            duration_ms = 240
            urgent_on_unfocused = false

            [bell.command]
            program = "canberra-gtk-play"
            args = ["--id", "bell"]
            "#,
        )
        .unwrap();

        assert_eq!(config.bell.duration_ms, 240);
        assert!(!config.bell.urgent_on_unfocused);
        let command = config.bell.command.unwrap();
        assert_eq!(command.program, "canberra-gtk-play");
        assert_eq!(command.args, ["--id", "bell"]);
    }

    #[test]
    fn parses_osc52_security_policy() {
        let config: GerminalConfig = toml::from_str(
            r#"
            [terminal]
            osc52 = "CopyPaste"
            "#,
        )
        .unwrap();

        assert_eq!(config.terminal_osc52_mode(), TerminalOsc52Mode::CopyPaste);
    }

    #[cfg(unix)]
    #[test]
    fn parses_shell_command_and_working_directory() {
        let config: GerminalConfig = toml::from_str(
            r#"
            [terminal]
            working_directory = "/tmp"

            [terminal.shell]
            program = "/bin/sh"
            args = ["-l"]
            "#,
        )
        .unwrap();

        assert_eq!(config.configured_working_directory(), Some("/tmp".into()));
        let shell = config.pty_shell_command().unwrap();
        assert_eq!(shell.program, "/bin/sh");
        assert_eq!(shell.args, ["-l"]);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn rejects_invalid_shell_and_working_directory_configuration() {
        let mut config = GerminalConfig::default();
        config.terminal.working_directory = Some("/path/that/does/not/exist".into());
        assert!(config.validate().unwrap_err().contains("working_directory"));

        config.terminal.working_directory = None;
        config.terminal.shell = Some(ShellConfig {
            program: "definitely-not-a-germinal-test-program".to_string(),
            args: Vec::new(),
        });
        assert!(
            config
                .validate()
                .unwrap_err()
                .contains("terminal.shell.program")
        );
    }

    #[test]
    fn terminal_profile_uses_configured_font() {
        let config: GerminalConfig = toml::from_str(
            r#"
            font.normal = { family = "JetBrains Mono", style = "Regular" }
            font.bold = { family = "JetBrains Mono", style = "Bold" }
            font.italic = { family = "JetBrains Mono", style = "Italic" }
            font.bold_italic = { family = "JetBrains Mono", style = "Bold Italic" }
            font.fallback = ["Noto Sans Mono CJK SC", "Symbols Nerd Font Mono"]
            font.size = 18
            "#,
        )
        .unwrap();
        let profile = config.terminal_profile();

        assert_eq!(profile.font_family().name(), "JetBrains Mono");
        assert_eq!(profile.font_size().logical_px(), 18.0);
        let font = profile.font_config();
        assert_eq!(font.normal().style(), Some("Regular"));
        assert_eq!(font.bold().and_then(|face| face.style()), Some("Bold"));
        assert_eq!(font.italic().and_then(|face| face.style()), Some("Italic"));
        assert_eq!(
            font.bold_italic().and_then(|face| face.style()),
            Some("Bold Italic")
        );
        assert_eq!(
            font.fallbacks()
                .iter()
                .map(|family| family.name())
                .collect::<Vec<_>>(),
            ["Noto Sans Mono CJK SC", "Symbols Nerd Font Mono"]
        );
    }

    #[test]
    fn rejects_empty_font_family_style_and_fallback() {
        for contents in [
            r#"font.normal = { family = "" }"#,
            r#"font.normal = { family = "monospace", style = "" }"#,
            r#"font.fallback = [""]"#,
        ] {
            let config: GerminalConfig = toml::from_str(contents).unwrap();
            assert!(config.validate().is_err());
        }
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
        assert_eq!(config.keyboard.bindings.len(), 24);
        assert_eq!(
            config.keyboard.bindings[0].action,
            KeyboardAction::ToggleViMode
        );
        assert_eq!(
            config.keyboard.bindings[1].action,
            KeyboardAction::ToggleSearch
        );
        assert_eq!(config.keyboard.bindings[1].key, "F");
        assert_eq!(config.keyboard.bindings[1].mods, "Control|Shift");
        assert_eq!(
            config.keyboard.bindings[2].action,
            KeyboardAction::SplitHorizontal
        );
        assert_eq!(config.keyboard.bindings[2].key, "D");
        assert_eq!(config.keyboard.bindings[2].mods, "Control|Shift");
        assert!(config.keyboard.bindings.iter().any(|binding| {
            binding.key == "Left"
                && binding.mods == "Alt|Shift"
                && binding.action == KeyboardAction::ResizePaneLeft
        }));
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
