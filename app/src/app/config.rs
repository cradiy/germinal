use std::{
    collections::BTreeMap,
    env, fs,
    path::{Component, Path, PathBuf},
};

use germinal_infra::rendering::pty_surface::{
    background_shader_renderer::WgpuBackgroundShaderSource,
    window_runtime::{
        DEFAULT_BACKGROUND_SHADER_MAX_FPS, WgpuTerminalPowerPreference,
        detect_terminal_power_preference,
    },
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
use serde::{Deserialize, Deserializer, Serialize};

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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GerminalConfig {
    pub window: WindowConfig,
    pub rendering: RenderingConfig,
    #[serde(skip_serializing_if = "BackgroundConfig::is_disabled")]
    pub background: BackgroundConfig,
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
        .with_faces(normal, bold, italic, bold_italic, fallbacks)
        .with_ligatures(self.font.ligatures);

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

    pub fn background_shader(&self) -> Option<WgpuBackgroundShaderSource> {
        self.background.resolved.clone()
    }

    pub fn terminal_power_preference(&self) -> WgpuTerminalPowerPreference {
        self.rendering.power_preference.into()
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
        if self.background.max_fps == 0 || self.background.max_fps > 1_000 {
            return Err("background.max_fps must be between 1 and 1000".to_string());
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

        for (index, binding) in self.keyboard.bindings.iter().enumerate() {
            if binding.key.trim().is_empty() {
                return Err(format!("keyboard.bindings[{index}].key must not be empty"));
            }
            for modifier in binding
                .mods
                .split('|')
                .map(str::trim)
                .filter(|modifier| !modifier.is_empty())
            {
                if !matches!(
                    modifier.to_ascii_lowercase().as_str(),
                    "control" | "alt" | "shift" | "super"
                ) {
                    return Err(format!(
                        "keyboard.bindings[{index}].mods contains unsupported modifier {modifier:?}; expected Control, Alt, Shift, or Super"
                    ));
                }
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RenderingConfig {
    pub power_preference: GpuPowerPreference,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GpuPowerPreference {
    #[default]
    High,
    Low,
}

impl From<GpuPowerPreference> for WgpuTerminalPowerPreference {
    fn from(preference: GpuPowerPreference) -> Self {
        match preference {
            GpuPowerPreference::High => Self::High,
            GpuPowerPreference::Low => Self::Low,
        }
    }
}

impl From<WgpuTerminalPowerPreference> for GpuPowerPreference {
    fn from(preference: WgpuTerminalPowerPreference) -> Self {
        match preference {
            WgpuTerminalPowerPreference::High => Self::High,
            WgpuTerminalPowerPreference::Low => Self::Low,
        }
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorsConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<PathBuf>,
    #[serde(flatten)]
    pub overrides: BTreeMap<String, String>,
    #[serde(skip)]
    resolved: TerminalColorTheme,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BackgroundConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shader: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub animated: Option<bool>,
    pub max_fps: u32,
    pub pause_when_unfocused: bool,
    #[serde(skip)]
    resolved: Option<WgpuBackgroundShaderSource>,
}

impl Default for BackgroundConfig {
    fn default() -> Self {
        Self {
            shader: None,
            animated: None,
            max_fps: DEFAULT_BACKGROUND_SHADER_MAX_FPS,
            pause_when_unfocused: true,
            resolved: None,
        }
    }
}

impl BackgroundConfig {
    fn is_disabled(&self) -> bool {
        self.shader.is_none()
    }

    fn resolve(&mut self, config_dir: &Path) -> Result<(), String> {
        let Some(shader) = self.shader.as_deref() else {
            self.resolved = None;
            return Ok(());
        };

        if shader == Path::new("starfield") {
            let animated = self.animated.unwrap_or(true);
            self.resolved = Some(WgpuBackgroundShaderSource::starfield().with_animated(animated));
            return Ok(());
        }

        let shader_path = expand_home(shader);
        let shader_path = if shader_path.is_absolute() {
            shader_path
        } else {
            config_dir.join(shader_path)
        };
        let source = fs::read_to_string(&shader_path).map_err(|error| {
            format!(
                "failed to read background shader {}: {error}",
                shader_path.display()
            )
        })?;
        let animated = self.animated.unwrap_or(false);
        self.resolved = Some(WgpuBackgroundShaderSource::new(
            shader_path.display().to_string(),
            source,
            animated,
        ));
        Ok(())
    }
}

impl ColorsConfig {
    fn resolve(&mut self, config_dir: &Path) -> Result<(), String> {
        self.resolved = kitty_colors::resolve_color_theme(self, config_dir)?;
        Ok(())
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
    pub motion_duration_ms: u64,
    pub motion_on_input: bool,
    pub motion_on_enter: bool,
}

impl Default for CursorConfig {
    fn default() -> Self {
        Self {
            shape: CursorShape::Block,
            blinking: false,
            blink_interval_ms: 750,
            motion_duration_ms: 80,
            motion_on_input: true,
            motion_on_enter: true,
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
    pub maximized: bool,
    pub opacity: f32,
    pub decorations: bool,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            title: "Germinal".to_string(),
            width_px: 960,
            height_px: 540,
            maximized: false,
            opacity: 1.0,
            decorations: true,
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
    pub ligatures: bool,
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
            ligatures: true,
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

#[derive(Debug, Clone, Serialize)]
pub struct KeyboardConfig {
    pub use_default_bindings: bool,
    pub bindings: Vec<KeyboardBinding>,
}

impl Default for KeyboardConfig {
    fn default() -> Self {
        Self {
            use_default_bindings: true,
            bindings: vec![
                KeyboardBinding {
                    key: "N".to_string(),
                    mods: "Control|Shift".to_string(),
                    action: KeyboardAction::NewWindow,
                },
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
                    key: "C".to_string(),
                    mods: "Control|Shift".to_string(),
                    action: KeyboardAction::Copy,
                },
                KeyboardBinding {
                    key: "V".to_string(),
                    mods: "Control|Shift".to_string(),
                    action: KeyboardAction::Paste,
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

#[derive(Deserialize)]
struct KeyboardConfigFile {
    #[serde(default = "default_true")]
    use_default_bindings: bool,
    #[serde(default)]
    bindings: Vec<KeyboardBinding>,
}

impl<'de> Deserialize<'de> for KeyboardConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let file = KeyboardConfigFile::deserialize(deserializer)?;
        let bindings = if file.use_default_bindings {
            merge_keyboard_bindings(Self::default().bindings, file.bindings)
        } else {
            merge_keyboard_bindings(Vec::new(), file.bindings)
        };
        Ok(Self {
            use_default_bindings: file.use_default_bindings,
            bindings,
        })
    }
}

fn default_true() -> bool {
    true
}

fn merge_keyboard_bindings(
    mut bindings: Vec<KeyboardBinding>,
    overrides: Vec<KeyboardBinding>,
) -> Vec<KeyboardBinding> {
    for binding in overrides {
        if let Some(index) = bindings
            .iter()
            .position(|candidate| same_keyboard_trigger(candidate, &binding))
        {
            bindings[index] = binding;
        } else {
            bindings.push(binding);
        }
    }
    bindings
}

fn same_keyboard_trigger(left: &KeyboardBinding, right: &KeyboardBinding) -> bool {
    left.key.eq_ignore_ascii_case(&right.key)
        && normalized_modifiers(&left.mods) == normalized_modifiers(&right.mods)
}

fn normalized_modifiers(modifiers: &str) -> Vec<String> {
    let mut modifiers = modifiers
        .split('|')
        .map(str::trim)
        .filter(|modifier| !modifier.is_empty())
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    modifiers.sort_unstable();
    modifiers.dedup();
    modifiers
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
    NewWindow,
    ToggleViMode,
    ToggleSearch,
    Copy,
    Paste,
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
            console_level: LogLevel::Info,
            file_level: LogLevel::Info,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    #[default]
    Info,
    Warn,
    Error,
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
    config
        .background
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
    let config = first_run_config(detect_terminal_power_preference());
    let contents = toml::to_string_pretty(&config).map_err(AppError::SerializeConfig)?;
    fs::write(paths.config_file(), contents).map_err(|source| AppError::WriteConfig {
        path: paths.config_file().to_path_buf(),
        source,
    })
}

fn first_run_config(power_preference: WgpuTerminalPowerPreference) -> GerminalConfig {
    let mut config = GerminalConfig::default();
    config.rendering.power_preference = power_preference.into();
    config
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
    use std::{fs, path::Path, time::SystemTime};

    use germinal_infra::rendering::pty_surface::window_runtime::WgpuTerminalPowerPreference;
    use germinal_ports::pty_host::{
        cursor_style::{TerminalCursorShape, TerminalCursorStyle},
        font_family::TerminalFontFamily,
        terminal_clipboard::TerminalOsc52Mode,
    };
    use germinal_ports::rendering::tab_bar::TabBarPosition;

    use super::{
        BackgroundConfig, GerminalConfig, GpuPowerPreference, KeyboardAction, ShellConfig,
        first_run_config,
    };

    #[test]
    fn default_config_serializes_alacritty_style_fields() {
        let contents = toml::to_string_pretty(&GerminalConfig::default()).unwrap();

        let value: toml::Value = toml::from_str(&contents).unwrap();
        assert_eq!(value["window"]["opacity"].as_float(), Some(1.0));
        assert_eq!(value["window"]["decorations"].as_bool(), Some(true));
        assert_eq!(value["window"]["maximized"].as_bool(), Some(false));
        assert_eq!(
            value["rendering"]["power_preference"].as_str(),
            Some("high")
        );
        assert!(value.get("background").is_none());
        assert_eq!(value["font"]["size"].as_float(), Some(16.0));
        assert_eq!(value["font"]["ligatures"].as_bool(), Some(true));
        assert_eq!(
            value["font"]["normal"]["family"].as_str(),
            Some(TerminalFontFamily::default().name())
        );
        assert_eq!(value["scrolling"]["history"].as_integer(), Some(10_000));
        assert_eq!(value["cursor"]["shape"].as_str(), Some("block"));
        assert_eq!(value["cursor"]["blinking"].as_bool(), Some(false));
        assert_eq!(value["cursor"]["blink_interval_ms"].as_integer(), Some(750));
        assert_eq!(value["cursor"]["motion_duration_ms"].as_integer(), Some(80));
        assert_eq!(value["cursor"]["motion_on_input"].as_bool(), Some(true));
        assert_eq!(value["cursor"]["motion_on_enter"].as_bool(), Some(true));
        assert_eq!(value["terminal"]["osc52"].as_str(), Some("OnlyCopy"));
        assert_eq!(value["bell"]["duration_ms"].as_integer(), Some(150));
        assert_eq!(value["bell"]["urgent_on_unfocused"].as_bool(), Some(true));
        assert_eq!(value["tabs"]["position"].as_str(), Some("bottom"));
        assert_eq!(value["logging"]["console_level"].as_str(), Some("info"));
        assert_eq!(value["logging"]["file_level"].as_str(), Some("info"));
        assert_eq!(
            value["keyboard"]["use_default_bindings"].as_bool(),
            Some(true)
        );
        assert!(
            value["keyboard"]["bindings"]
                .as_array()
                .is_some_and(|bindings| bindings
                    .iter()
                    .any(|binding| { binding["action"].as_str() == Some("ToggleViMode") }))
        );

        let round_trip: GerminalConfig = toml::from_str(&contents).unwrap();
        assert!(round_trip.keyboard.use_default_bindings);
        assert_eq!(round_trip.keyboard.bindings.len(), 27);
    }

    #[test]
    fn parses_rendering_power_preference() {
        let config: GerminalConfig = toml::from_str(
            r#"
            [rendering]
            power_preference = "low"
            "#,
        )
        .unwrap();

        assert_eq!(config.rendering.power_preference, GpuPowerPreference::Low);
        assert_eq!(
            config.terminal_power_preference(),
            WgpuTerminalPowerPreference::Low
        );
    }

    #[test]
    fn first_run_config_uses_detected_power_preference() {
        let config = first_run_config(WgpuTerminalPowerPreference::Low);

        assert_eq!(config.rendering.power_preference, GpuPowerPreference::Low);
    }

    #[test]
    fn parses_cursor_configuration() {
        let config: GerminalConfig = toml::from_str(
            r#"
            [cursor]
            shape = "beam"
            blinking = true
            blink_interval_ms = 320
            motion_duration_ms = 65
            motion_on_input = false
            motion_on_enter = true
            "#,
        )
        .unwrap();

        assert_eq!(
            config.terminal_cursor_style(),
            TerminalCursorStyle::new(TerminalCursorShape::Beam, true)
        );
        assert_eq!(config.cursor.blink_interval_ms, 320);
        assert_eq!(config.cursor.motion_duration_ms, 65);
        assert!(!config.cursor.motion_on_input);
        assert!(config.cursor.motion_on_enter);
    }

    #[test]
    fn parses_window_startup_configuration() {
        let config: GerminalConfig = toml::from_str(
            r#"
            [window]
            decorations = false
            maximized = true
            "#,
        )
        .unwrap();

        assert!(!config.window.decorations);
        assert!(config.window.maximized);
    }

    #[test]
    fn resolves_built_in_starfield_background() {
        let mut background: BackgroundConfig = toml::from_str(
            r#"
            shader = "starfield"
            "#,
        )
        .unwrap();

        background.resolve(Path::new("/unused")).unwrap();

        assert_eq!(background.max_fps, 60);
        assert!(background.pause_when_unfocused);
        let shader = background.resolved.unwrap();
        assert_eq!(shader.label(), "starfield");
        assert!(shader.animated());
        assert!(shader.source().contains("fn background("));
    }

    #[test]
    fn resolves_relative_custom_background_shader() {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let config_dir = std::env::temp_dir().join(format!(
            "germinal-background-config-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(config_dir.join("shaders")).unwrap();
        let shader_path = config_dir.join("shaders/custom.wgsl");
        fs::write(
            &shader_path,
            "fn background(uv: vec2<f32>, time: f32, resolution: vec2<f32>) -> vec4<f32> { return vec4<f32>(uv, time / resolution.x, 1.0); }",
        )
        .unwrap();
        let mut background: BackgroundConfig = toml::from_str(
            r#"
            shader = "shaders/custom.wgsl"
            animated = true
            "#,
        )
        .unwrap();

        background.resolve(&config_dir).unwrap();

        let shader = background.resolved.unwrap();
        assert_eq!(shader.label(), shader_path.display().to_string());
        assert!(shader.animated());
        assert!(shader.source().contains("return vec4<f32>"));
        fs::remove_dir_all(config_dir).unwrap();
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
    fn parses_and_validates_background_shader_frame_rate() {
        let config: GerminalConfig = toml::from_str(
            r#"
            [background]
            shader = "starfield"
            max_fps = 48
            pause_when_unfocused = false
            "#,
        )
        .unwrap();

        assert_eq!(config.background.max_fps, 48);
        assert!(!config.background.pause_when_unfocused);
        assert!(config.validate().is_ok());

        let mut config = GerminalConfig::default();
        config.background.max_fps = 0;
        assert!(
            config
                .validate()
                .unwrap_err()
                .contains("background.max_fps")
        );
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
            font.normal = { family = "Example Mono", style = "Regular" }
            font.bold = { family = "Example Mono", style = "Bold" }
            font.italic = { family = "Example Mono", style = "Italic" }
            font.bold_italic = { family = "Example Mono", style = "Bold Italic" }
            font.fallback = ["Example CJK", "Example Symbols"]
            font.size = 18
            font.ligatures = false
            "#,
        )
        .unwrap();
        let profile = config.terminal_profile();

        assert_eq!(profile.font_family().name(), "Example Mono");
        assert_eq!(profile.font_size().logical_px(), 18.0);
        let font = profile.font_config();
        assert!(!font.ligatures());
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
            ["Example CJK", "Example Symbols"]
        );
    }

    #[test]
    fn rejects_empty_font_family_style_and_fallback() {
        for contents in [
            r#"font.normal = { family = "" }"#,
            r#"font.normal = { family = "Example Mono", style = "" }"#,
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
        assert_eq!(config.keyboard.bindings.len(), 27);
        assert_eq!(
            config.keyboard.bindings[1].action,
            KeyboardAction::ToggleViMode
        );
        assert_eq!(
            config.keyboard.bindings[2].action,
            KeyboardAction::ToggleSearch
        );
        assert_eq!(config.keyboard.bindings[2].key, "F");
        assert_eq!(config.keyboard.bindings[2].mods, "Control|Shift");
        assert!(config.keyboard.bindings.iter().any(|binding| {
            binding.key == "N"
                && binding.mods == "Control|Shift"
                && binding.action == KeyboardAction::NewWindow
        }));
        assert!(config.keyboard.bindings.iter().any(|binding| {
            binding.key == "C"
                && binding.mods == "Control|Shift"
                && binding.action == KeyboardAction::Copy
        }));
        assert!(config.keyboard.bindings.iter().any(|binding| {
            binding.key == "V"
                && binding.mods == "Control|Shift"
                && binding.action == KeyboardAction::Paste
        }));
        assert!(config.keyboard.bindings.iter().any(|binding| {
            binding.key == "D"
                && binding.mods == "Control|Shift"
                && binding.action == KeyboardAction::SplitHorizontal
        }));
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

        assert!(config.keyboard.use_default_bindings);
        assert_eq!(config.keyboard.bindings.len(), 27);
        let binding = config
            .keyboard
            .bindings
            .iter()
            .find(|binding| binding.key.eq_ignore_ascii_case("V"))
            .unwrap();
        assert_eq!(binding.key, "V");
        assert_eq!(binding.mods, "Control|Shift");
        assert_eq!(binding.action, KeyboardAction::ToggleViMode);
    }

    #[test]
    fn rejects_empty_binding_keys_and_unknown_modifiers() {
        for (contents, expected) in [
            (
                r#"
                [[keyboard.bindings]]
                key = ""
                action = "NewTab"
                "#,
                ".key must not be empty",
            ),
            (
                r#"
                [[keyboard.bindings]]
                key = "T"
                mods = "Control|Hyper"
                action = "NewTab"
                "#,
                "unsupported modifier",
            ),
        ] {
            let config: GerminalConfig = toml::from_str(contents).unwrap();
            let error = config.validate().unwrap_err();
            assert!(error.contains(expected), "unexpected error: {error}");
        }
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

        assert_eq!(config.keyboard.bindings.len(), 27);
        assert!(config.keyboard.bindings.iter().any(|binding| {
            binding.key == "D"
                && binding.mods == "Control|Shift"
                && binding.action == KeyboardAction::SplitHorizontal
        }));
        assert!(config.keyboard.bindings.iter().any(|binding| {
            binding.key == "W"
                && binding.mods == "Control|Shift"
                && binding.action == KeyboardAction::ClosePane
        }));
    }

    #[test]
    fn custom_keyboard_binding_overrides_one_trigger_and_keeps_other_defaults() {
        let config: GerminalConfig = toml::from_str(
            r#"
            [[keyboard.bindings]]
            key = "v"
            mods = "Shift|Control"
            action = "ToggleViMode"
            "#,
        )
        .unwrap();

        assert_eq!(config.keyboard.bindings.len(), 27);
        assert!(config.keyboard.bindings.iter().any(|binding| {
            binding.key.eq_ignore_ascii_case("V") && binding.action == KeyboardAction::ToggleViMode
        }));
        assert!(config.keyboard.bindings.iter().any(|binding| {
            binding.key == "C"
                && binding.mods == "Control|Shift"
                && binding.action == KeyboardAction::Copy
        }));
    }

    #[test]
    fn one_action_can_have_multiple_keyboard_triggers() {
        let config: GerminalConfig = toml::from_str(
            r#"
            [[keyboard.bindings]]
            key = "F12"
            action = "ToggleViMode"

            [[keyboard.bindings]]
            key = "V"
            mods = "Control|Alt"
            action = "ToggleViMode"
            "#,
        )
        .unwrap();

        let vi_mode_bindings = config
            .keyboard
            .bindings
            .iter()
            .filter(|binding| binding.action == KeyboardAction::ToggleViMode)
            .collect::<Vec<_>>();
        assert_eq!(vi_mode_bindings.len(), 3);
        assert!(
            vi_mode_bindings
                .iter()
                .any(|binding| binding.key == "Space")
        );
        assert!(vi_mode_bindings.iter().any(|binding| binding.key == "F12"));
        assert!(
            vi_mode_bindings
                .iter()
                .any(|binding| binding.key == "V" && binding.mods == "Control|Alt")
        );
    }

    #[test]
    fn default_keyboard_bindings_can_be_disabled_explicitly() {
        let config: GerminalConfig = toml::from_str(
            r#"
            [keyboard]
            use_default_bindings = false

            [[keyboard.bindings]]
            key = "V"
            mods = "Control|Shift"
            action = "ToggleViMode"
            "#,
        )
        .unwrap();

        assert!(!config.keyboard.use_default_bindings);
        assert_eq!(config.keyboard.bindings.len(), 1);
        assert_eq!(
            config.keyboard.bindings[0].action,
            KeyboardAction::ToggleViMode
        );
    }

    #[test]
    fn disabling_defaults_with_no_bindings_disables_all_host_shortcuts() {
        let config: GerminalConfig = toml::from_str(
            r#"
            [keyboard]
            use_default_bindings = false
            "#,
        )
        .unwrap();

        assert!(config.keyboard.bindings.is_empty());
    }
}
