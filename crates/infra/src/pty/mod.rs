pub mod portable_pty_bridge;
pub mod shell_command;

#[cfg(unix)]
mod unix_pty_bridge;
#[cfg(unix)]
pub mod unix_pty_backend;
#[cfg(windows)]
mod windows_pty_bridge;
#[cfg(windows)]
pub mod windows_pty_backend;

#[cfg(unix)]
pub use unix_pty_backend::PlatformPtyBackend;
#[cfg(windows)]
pub use windows_pty_backend::PlatformPtyBackend;
