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
	config_dir:  PathBuf,
	config_file: PathBuf,
	log_dir:     PathBuf,
}

impl AppPaths {
	pub fn resolve() -> AppResult<Self> {
		let config_dir = xdg_dir("XDG_CONFIG_HOME", ".config")?.join(APP_NAME);
		let state_dir = xdg_dir("XDG_STATE_HOME", ".local/state")?.join(APP_NAME);
		let config_file = config_dir.join(CONFIG_FILE_NAME);
		let log_dir = state_dir.join("logs");

		Ok(Self { config_dir, config_file, log_dir })
	}

	pub fn config_dir(&self) -> &Path { &self.config_dir }

	pub fn config_file(&self) -> &Path { &self.config_file }

	pub fn log_dir(&self) -> &Path { &self.log_dir }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GerminalConfig {
	pub window:   WindowConfig,
	pub terminal: TerminalConfig,
	pub logging:  LoggingConfig,
}

impl GerminalConfig {
	pub fn terminal_profile(&self) -> TerminalProfile {
		let default_profile = TerminalProfile::DEFAULT;
		let size_config = TerminalSizeConfig::new(
			default_profile.size_config().cell_size(),
			TerminalPadding::new(self.terminal.padding_x_px, self.terminal.padding_y_px),
			self.terminal.dynamic_padding,
		);

		TerminalProfile::new(
			TerminalFontFamily::DEFAULT,
			TerminalFontSize::from_points(self.terminal.font_size_points),
			size_config,
		)
	}
}

impl Default for GerminalConfig {
	fn default() -> Self {
		Self {
			window:   WindowConfig::default(),
			terminal: TerminalConfig::default(),
			logging:  LoggingConfig::default(),
		}
	}
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowConfig {
	pub title:     String,
	pub width_px:  u32,
	pub height_px: u32,
}

impl Default for WindowConfig {
	fn default() -> Self { Self { title: "Germinal".to_string(), width_px: 960, height_px: 540 } }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalConfig {
	pub font_size_points: f32,
	pub padding_x_px:     u32,
	pub padding_y_px:     u32,
	pub dynamic_padding:  bool,
}

impl Default for TerminalConfig {
	fn default() -> Self {
		Self {
			font_size_points: 16.0,
			padding_x_px:     0,
			padding_y_px:     0,
			dynamic_padding:  false,
		}
	}
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
	pub console_level: LogLevel,
	pub file_level:    LogLevel,
}

impl Default for LoggingConfig {
	fn default() -> Self { Self { console_level: LogLevel::Debug, file_level: LogLevel::Info } }
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
	fn default() -> Self { Self::Info }
}

pub fn load_or_create_config() -> AppResult<(GerminalConfig, AppPaths)> {
	let paths = AppPaths::resolve()?;
	if !paths.config_file().exists() {
		write_default_config(&paths)?;
	}

	let contents = fs::read_to_string(paths.config_file())
		.map_err(|source| AppError::ReadConfig { path: paths.config_file().to_path_buf(), source })?;
	let config = toml::from_str(&contents)
		.map_err(|source| AppError::ParseConfig { path: paths.config_file().to_path_buf(), source })?;

	Ok((config, paths))
}

fn write_default_config(paths: &AppPaths) -> AppResult<()> {
	create_dir_all(paths.config_dir())?;
	let contents =
		toml::to_string_pretty(&GerminalConfig::default()).map_err(AppError::SerializeConfig)?;
	fs::write(paths.config_file(), contents)
		.map_err(|source| AppError::WriteConfig { path: paths.config_file().to_path_buf(), source })
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
	fs::create_dir_all(path)
		.map_err(|source| AppError::CreateDirectory { path: path.to_path_buf(), source })
}
