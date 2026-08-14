use std::{
    fs::OpenOptions,
    io::{self, Write},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use tracing_subscriber::{
    Layer, filter::LevelFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt,
};

use crate::app::{
    config::{AppPaths, LogLevel, LoggingConfig, create_dir_all},
    error::{AppError, AppResult},
};

pub fn init_logging(config: &LoggingConfig, paths: &AppPaths) -> AppResult<()> {
    create_dir_all(paths.log_dir())?;

    let log_file_path = paths.log_dir().join(startup_log_file_name());
    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_file_path)
        .map_err(|source| AppError::OpenLogFile {
            path: log_file_path.clone(),
            source,
        })?;
    let file_writer = SharedFileWriter::new(log_file);

    tracing_subscriber::registry()
        .with(
            fmt::layer()
                .with_writer(io::stderr)
                .with_filter(level_filter(config.console_level)),
        )
        .with(
            fmt::layer()
                .with_ansi(false)
                .with_writer(move || file_writer.clone())
                .with_filter(level_filter(config.file_level)),
        )
        .try_init()
        .map_err(AppError::LoggingInit)
}

fn level_filter(level: LogLevel) -> LevelFilter {
    match level {
        LogLevel::Trace => LevelFilter::TRACE,
        LogLevel::Debug => LevelFilter::DEBUG,
        LogLevel::Info => LevelFilter::INFO,
        LogLevel::Warn => LevelFilter::WARN,
        LogLevel::Error => LevelFilter::ERROR,
    }
}

fn startup_log_file_name() -> String {
    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("germinal-{timestamp_ms}.log")
}

#[derive(Clone)]
struct SharedFileWriter {
    file: Arc<Mutex<std::fs::File>>,
}

impl SharedFileWriter {
    fn new(file: std::fs::File) -> Self {
        Self {
            file: Arc::new(Mutex::new(file)),
        }
    }
}

impl Write for SharedFileWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut file = match self.file.lock() {
            Ok(file) => file,
            Err(poisoned) => poisoned.into_inner(),
        };
        file.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut file = match self.file.lock() {
            Ok(file) => file,
            Err(poisoned) => poisoned.into_inner(),
        };
        file.flush()
    }
}
