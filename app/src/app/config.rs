#[cfg(not(windows))]
use std::env;
use std::{
    collections::BTreeMap,
    fs,
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
use germinal_ports::rendering::tab_bar::{TabBarPosition, TabBarStyle};
use serde::{Deserialize, Deserializer, Serialize};

use crate::app::error::{AppError, AppResult};

mod kitty_colors;

pub const APP_NAME: &str = "germinal";

const CONFIG_FILE_NAME: &str = "config.toml";

#[derive(Debug, Clone)]
pub struct AppPaths {
    config_dir: PathBuf,
    config_file: PathBuf,
    state_dir: PathBuf,
    log_dir: PathBuf,
}

impl AppPaths {
    pub fn resolve() -> AppResult<Self> {
        let config_dir = dirs::config_dir()
            .ok_or(AppError::MissingPlatformDirectory("configuration"))?
            .join(APP_NAME);
        let state_dir = dirs::state_dir()
            .or_else(dirs::data_local_dir)
            .ok_or(AppError::MissingPlatformDirectory("state"))?
            .join(APP_NAME);
        let config_file = config_dir.join(CONFIG_FILE_NAME);
        let log_dir = state_dir.join("logs");

        Ok(Self {
            config_dir,
            config_file,
            state_dir,
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

    pub fn ui_state_file(&self) -> PathBuf {
        self.state_dir.join("ui-state.toml")
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

    pub(super) fn effective_tab_position(&self) -> TabBarPosition {
        if self.window.modern {
            TabBarPosition::Left
        } else {
            self.tabs.position
        }
    }

    pub(super) fn effective_titlebar(&self) -> TitlebarMode {
        if self.window.modern {
            TitlebarMode::Custom
        } else {
            self.window.titlebar
        }
    }

    pub(super) fn effective_tab_style(&self) -> TabBarStyle {
        if self.window.modern {
            TabBarStyle::Fade
        } else {
            self.tabs.style
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.tabs.position == TabBarPosition::Left {
            return Err(
                "tabs.position must be top or bottom; use window.modern for the modern layout"
                    .into(),
            );
        }
        if !(1..=32).contains(&self.scrolling.multiplier) {
            return Err("scrolling.multiplier must be between 1 and 32".into());
        }
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
            #[cfg(windows)]
            germinal_infra::pty::windows_shell::command(
                &shell.program,
                &shell.args,
                self.configured_working_directory().as_deref(),
            )
            .map_err(|error| format!("terminal.shell.program: {error}"))?;
            #[cfg(not(windows))]
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

    let Some(home) = dirs::home_dir() else {
        return path.to_path_buf();
    };
    home.join(components.as_path())
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
    pub style: TabBarStyle,
}

impl Default for TabsConfig {
    fn default() -> Self {
        Self {
            position: TabBarPosition::Top,
            style: TabBarStyle::Fade,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TitlebarMode {
    Native,
    Hidden,
    #[default]
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WindowConfig {
    pub modern: bool,
    pub titlebar: TitlebarMode,
    pub show_window_controls: bool,
    pub title: String,
    pub width_px: u32,
    pub height_px: u32,
    pub maximized: bool,
    pub opacity: f32,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            modern: false,
            titlebar: TitlebarMode::Custom,
            show_window_controls: true,
            title: "Germinal".to_string(),
            width_px: 960,
            height_px: 540,
            maximized: false,
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
    pub multiplier: u8,
}

impl Default for ScrollingConfig {
    fn default() -> Self {
        Self {
            history: 10_000,
            multiplier: 1,
        }
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
                KeyboardBinding {
                    key: "P".to_string(),
                    mods: "Control|Shift".to_string(),
                    action: KeyboardAction::CommandPalette,
                },
                KeyboardBinding {
                    key: "B".to_string(),
                    mods: "Control|Alt".to_string(),
                    action: KeyboardAction::ToggleTabSidebar,
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
    CommandPalette,
    ToggleTabSidebar,
    #[serde(alias = "ToggleToolbar")]
    ToggleTitlebar,
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

pub(super) fn save_palette_settings(
    config: &GerminalConfig,
    fields: &[(&str, &str)],
) -> Result<(), String> {
    use std::io::Write;
    let paths = AppPaths::resolve().map_err(|error| error.to_string())?;
    let path = paths.config_file();
    let source = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let updated = update_palette_settings(&source, config, fields)?;
    let mut file =
        tempfile::NamedTempFile::new_in(paths.config_dir()).map_err(|error| error.to_string())?;
    file.as_file()
        .set_permissions(
            fs::metadata(path)
                .map_err(|error| error.to_string())?
                .permissions(),
        )
        .map_err(|error| error.to_string())?;
    file.write_all(updated.as_bytes())
        .map_err(|error| error.to_string())?;
    file.as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    // Avoid replacing edits made while this command was preparing its update.
    if fs::read_to_string(path).map_err(|error| error.to_string())? != source {
        return Err("Configuration changed on disk; retry the selection".into());
    }
    file.persist(path).map_err(|error| error.to_string())?;
    Ok(())
}

fn update_palette_settings(
    source: &str,
    config: &GerminalConfig,
    fields: &[(&str, &str)],
) -> Result<String, String> {
    toml::from_str::<GerminalConfig>(source).map_err(|error| error.to_string())?;
    let mut document = source
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| error.to_string())?;
    let values = toml::to_string(config)
        .map_err(|error| error.to_string())?
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| error.to_string())?;
    for &(section, key) in fields {
        if document.get(section).is_none() {
            document[section] = toml_edit::Item::Table(toml_edit::Table::new());
        }
        let mut value = values[section][key].clone();
        if let (Some(old), Some(new)) = (
            document[section]
                .get(key)
                .and_then(toml_edit::Item::as_value),
            value.as_value_mut(),
        ) {
            *new.decor_mut() = old.decor().clone();
        }
        document[section][key] = value;
    }
    let result = document.to_string();
    toml::from_str::<GerminalConfig>(&result).map_err(|error| error.to_string())?;
    Ok(result)
}

pub fn load_or_create_config() -> AppResult<(GerminalConfig, AppPaths, Option<String>)> {
    let paths = AppPaths::resolve()?;
    if !paths.config_file().exists() {
        write_default_config(&paths)?;
    }

    let (config, warning) = load_config_or_default(&paths);
    Ok((config, paths, warning))
}

fn load_config_or_default(paths: &AppPaths) -> (GerminalConfig, Option<String>) {
    match read_config(paths) {
        Ok(config) => (config, None),
        Err(error) => (
            GerminalConfig::default(),
            Some(format!(
                "{error}; using default configuration for this session. The configuration file has not been changed."
            )),
        ),
    }
}

fn read_config(paths: &AppPaths) -> AppResult<GerminalConfig> {
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

    Ok(config)
}

#[cfg(not(windows))]
fn program_exists(program: &str) -> bool {
    let path = Path::new(program);
    if path.is_absolute() || path.components().count() > 1 {
        return is_executable_file(path);
    }

    env::split_paths(&env::var_os("PATH").unwrap_or_default())
        .map(|directory| directory.join(program))
        .any(|candidate| is_executable_file(&candidate))
}

#[cfg(not(windows))]
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
        terminal_clipboard::TerminalOsc52Mode,
    };
    use germinal_ports::rendering::tab_bar::{TabBarPosition, TabBarStyle};

    use super::{
        BackgroundConfig, GerminalConfig, GpuPowerPreference, KeyboardAction, ShellConfig,
    };

    #[test]
    fn default_config_round_trip_does_not_duplicate_keyboard_bindings() {
        let contents = toml::to_string_pretty(&GerminalConfig::default()).unwrap();

        let round_trip: GerminalConfig = toml::from_str(&contents).unwrap();
        assert!(round_trip.keyboard.use_default_bindings);
        assert_eq!(
            round_trip.keyboard.bindings.len(),
            super::KeyboardConfig::default().bindings.len()
        );
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
    fn modern_layout_overrides_preferences_until_switched_off() {
        let mut config = GerminalConfig::default();
        config.window.modern = true;
        config.window.titlebar = super::TitlebarMode::Hidden;
        config.tabs.position = TabBarPosition::Bottom;
        assert_eq!(config.effective_titlebar(), super::TitlebarMode::Custom);
        assert_eq!(config.effective_tab_position(), TabBarPosition::Left);
        // Preferences can change while modern mode is active, without changing its layout.
        config.window.titlebar = super::TitlebarMode::Native;
        config.tabs.position = TabBarPosition::Top;
        assert_eq!(config.effective_titlebar(), super::TitlebarMode::Custom);
        assert_eq!(config.effective_tab_position(), TabBarPosition::Left);
        config.window.modern = false;
        assert_eq!(config.effective_titlebar(), super::TitlebarMode::Native);
        assert_eq!(config.effective_tab_position(), TabBarPosition::Top);
    }

    #[test]
    fn invalid_config_falls_back_without_overwriting_the_file() {
        let directory = tempfile::tempdir().unwrap();
        let paths = super::AppPaths {
            config_dir: directory.path().into(),
            config_file: directory.path().join("config.toml"),
            state_dir: directory.path().into(),
            log_dir: directory.path().into(),
        };
        for source in [
            "[broken",
            "[window]\ndecorations = true",
            "[window]\nopacity = 2.0",
        ] {
            std::fs::write(paths.config_file(), source).unwrap();
            let (config, warning) = super::load_config_or_default(&paths);
            assert!(warning.unwrap().contains("using default configuration"));
            assert!(config.validate().is_ok());
            assert_eq!(
                std::fs::read_to_string(paths.config_file()).unwrap(),
                source
            );
        }
        std::fs::write(paths.config_file(), "[window]\nopacity = 0.7").unwrap();
        let (config, warning) = super::load_config_or_default(&paths);
        assert!(warning.is_none());
        assert_eq!(config.window.opacity, 0.7);
    }

    #[test]
    fn parses_window_startup_configuration() {
        let config: GerminalConfig = toml::from_str(
            r#"
            [window]
            titlebar = "hidden"
            maximized = true
            "#,
        )
        .unwrap();

        assert_eq!(config.window.titlebar, super::TitlebarMode::Hidden);
        assert!(config.window.maximized);
    }

    #[test]
    fn palette_save_preserves_unrelated_settings_and_comments() {
        let source = "# personal settings\n[window]\ntitlebar = 'native' # preferred frame\nopacity = 0.8\n\n[tabs]\nposition = 'bottom'\n";
        let config = GerminalConfig::default();
        let updated = super::update_palette_settings(
            source,
            &config,
            &[("window", "titlebar"), ("tabs", "style")],
        )
        .unwrap();
        assert!(updated.contains("# personal settings"));
        assert!(updated.contains("# preferred frame"));
        assert!(updated.contains("opacity = 0.8"));
        assert!(updated.contains("position = 'bottom'"));
        let parsed: GerminalConfig = toml::from_str(&updated).unwrap();
        assert_eq!(parsed.window.titlebar, super::TitlebarMode::Custom);
        assert!(
            super::update_palette_settings("[broken", &config, &[("window", "titlebar")]).is_err()
        );
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
    fn parses_all_tab_bar_styles() {
        for (value, expected) in [
            ("fade", TabBarStyle::Fade),
            ("powerline", TabBarStyle::Powerline),
        ] {
            let config: GerminalConfig = toml::from_str(&format!(
                r#"
                [tabs]
                style = "{value}"
                "#,
            ))
            .unwrap();

            assert_eq!(config.tabs.style, expected);
        }
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
    fn validates_scroll_multiplier_and_preserves_old_configs() {
        let config: GerminalConfig = toml::from_str("[scrolling]\nhistory = 512").unwrap();
        assert_eq!(config.scrolling.multiplier, 1);
        for value in [0, 1, 3, 32, 33] {
            let config: GerminalConfig =
                toml::from_str(&format!("[scrolling]\nmultiplier = {value}")).unwrap();
            assert_eq!(config.validate().is_ok(), (1..=32).contains(&value));
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
        assert_eq!(
            config.keyboard.bindings.len(),
            super::KeyboardConfig::default().bindings.len()
        );
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
        assert_eq!(
            config.keyboard.bindings.len(),
            super::KeyboardConfig::default().bindings.len()
        );
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

        assert_eq!(
            config.keyboard.bindings.len(),
            super::KeyboardConfig::default().bindings.len()
        );
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

        assert_eq!(
            config.keyboard.bindings.len(),
            super::KeyboardConfig::default().bindings.len()
        );
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
