use std::{
    backtrace::Backtrace,
    fmt::{Debug, Display},
    fs::OpenOptions,
    io::{self, Write},
    panic,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Once},
    time::{SystemTime, UNIX_EPOCH},
};

use tracing_subscriber::{
    Layer, filter::LevelFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt,
};

use crate::app::{
    config::{AppPaths, LogLevel, LoggingConfig, create_dir_all},
    error::{AppError, AppResult},
};

const CRASH_LOG_FILE_NAME: &str = "germinal-crash.log";
static PANIC_HOOK: Once = Once::new();

pub fn prepare_crash_reporting() {
    let Ok(paths) = AppPaths::resolve() else {
        return;
    };
    if create_dir_all(paths.log_dir()).is_err() {
        return;
    }
    install_panic_hook(paths.log_dir().join(CRASH_LOG_FILE_NAME));
}

pub fn report_fatal_error(error: &(impl Debug + Display)) {
    tracing::error!(error = ?error, "Germinal terminated with a fatal error");

    let Ok(paths) = AppPaths::resolve() else {
        return;
    };
    if create_dir_all(paths.log_dir()).is_err() {
        return;
    }
    write_fatal_error_record(&paths.log_dir().join(CRASH_LOG_FILE_NAME), error);
}

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
        .map_err(AppError::LoggingInit)?;

    install_panic_hook(paths.log_dir().join(CRASH_LOG_FILE_NAME));
    Ok(())
}

fn install_panic_hook(crash_log_path: PathBuf) {
    PANIC_HOOK.call_once(move || {
        let default_hook = panic::take_hook();
        panic::set_hook(Box::new(move |panic_info| {
            write_crash_record(&crash_log_path, panic_info);
            default_hook(panic_info);
        }));
    });
}

fn write_crash_record(path: &Path, panic_info: &panic::PanicHookInfo<'_>) {
    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let thread = std::thread::current();
    let thread_name = thread.name().unwrap_or("unnamed");
    let payload = panic_info
        .payload()
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| {
            panic_info
                .payload()
                .downcast_ref::<String>()
                .map(String::as_str)
        })
        .unwrap_or("non-string panic payload");
    let location = panic_info
        .location()
        .map(|location| {
            format!(
                "{}:{}:{}",
                location.file(),
                location.line(),
                location.column()
            )
        })
        .unwrap_or_else(|| "unknown".to_owned());
    let backtrace = Backtrace::force_capture();

    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let _ = writeln!(
        file,
        "timestamp_ms={timestamp_ms} pid={} thread={thread_name:?} location={location}\npanic: {payload}\nbacktrace:\n{backtrace}\n",
        std::process::id(),
    );
    let _ = file.flush();
}

fn write_fatal_error_record(path: &Path, error: &(impl Debug + Display)) {
    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let _ = writeln!(
        file,
        "timestamp_ms={timestamp_ms} pid={} fatal: {error}\ndetails: {error:?}\n",
        std::process::id(),
    );
    let _ = file.flush();
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

#[cfg(test)]
mod tests {
    use std::{fs, io};

    use super::write_fatal_error_record;

    #[test]
    fn fatal_errors_are_written_to_an_explicit_crash_log() {
        let test_dir = std::env::temp_dir().join(format!(
            "germinal-crash-log-test-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("unnamed")
        ));
        fs::create_dir_all(&test_dir).expect("test crash-log directory should be created");
        let crash_log = test_dir.join("crash.log");
        let error = io::Error::other("startup failed");

        write_fatal_error_record(&crash_log, &error);

        let contents = fs::read_to_string(&crash_log).expect("crash log should be readable");
        assert!(contents.contains("fatal: startup failed"));
        assert!(contents.contains("details:"));

        fs::remove_file(crash_log).expect("test crash log should be removed");
        fs::remove_dir(test_dir).expect("test crash-log directory should be removed");
    }
}
