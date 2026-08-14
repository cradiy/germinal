use std::{io, path::PathBuf};

use thiserror::Error;

use crate::app::paste::PasteError;

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("HOME is not set; cannot resolve Germinal XDG paths")]
    MissingHomeDir,
    #[error("failed to create directory {path}: {source}")]
    CreateDirectory {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to read config file {path}: {source}")]
    ReadConfig {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to parse config file {path}: {source}")]
    ParseConfig {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("failed to serialize config: {0}")]
    SerializeConfig(#[source] toml::ser::Error),
    #[error("failed to write config file {path}: {source}")]
    WriteConfig {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to open log file {path}: {source}")]
    OpenLogFile {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to initialize tracing subscriber: {0}")]
    LoggingInit(#[source] tracing_subscriber::util::TryInitError),
    #[error("failed to create media bridge: {0}")]
    MediaBridge(
        #[source] germinal_infra::gnative::gst_video_player_bridge::GstVideoPlayerBridgeError,
    ),
    #[error("failed to create gnative tunnel: {0}")]
    CreateGNativeTunnel(#[source] germinal_ports::service::gnative_tunnel::GNativeTunnelError),
    #[error("failed to restore workspace: {0}")]
    RestoreWorkspace(#[source] germinal_ports::service::workspace_service::WorkspaceServiceError),
    #[error("failed to run Germinal event loop: {0}")]
    RunEventLoop(#[source] winit::error::EventLoopError),
    #[error("failed to create Germinal window: {0}")]
    CreateWindow(#[source] winit::error::OsError),
    #[error("failed to create Germinal window runtime: {0}")]
    CreateWindowRuntime(
        #[source] germinal_infra::rendering::pty_surface::window_runtime::WindowRuntimeError,
    ),
    #[error(transparent)]
    Paste(#[from] PasteError),
}
