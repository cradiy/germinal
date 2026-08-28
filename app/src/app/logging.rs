use std::{
    backtrace::Backtrace,
    fmt::{Debug, Display},
    fs::{self, OpenOptions},
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
const PREVIOUS_CRASH_LOG_FILE_NAME: &str = "germinal-crash.previous.log";
const MAX_STARTUP_LOG_FILES: usize = 20;
const MAX_CRASH_LOG_BYTES: u64 = 4 * 1024 * 1024;
static PANIC_HOOK: Once = Once::new();

pub fn prepare_crash_reporting() {
    let Ok(paths) = AppPaths::resolve() else {
        return;
    };
    if create_dir_all(paths.log_dir()).is_err() {
        return;
    }
    rotate_crash_log(paths.log_dir());
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
    prune_startup_logs(paths.log_dir(), MAX_STARTUP_LOG_FILES.saturating_sub(1));

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

fn prune_startup_logs(log_dir: &Path, keep: usize) {
    let Ok(entries) = fs::read_dir(log_dir) else {
        return;
    };
    let mut logs = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            startup_log_timestamp(name).map(|timestamp| (timestamp, entry.path()))
        })
        .collect::<Vec<_>>();
    logs.sort_unstable_by_key(|(timestamp, _)| *timestamp);
    let remove_count = logs.len().saturating_sub(keep);
    for (_, path) in logs.into_iter().take(remove_count) {
        let _ = fs::remove_file(path);
    }
}

fn startup_log_timestamp(file_name: &str) -> Option<u128> {
    let timestamp = file_name.strip_prefix("germinal-")?.strip_suffix(".log")?;
    if timestamp.is_empty() || !timestamp.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    timestamp.parse().ok()
}

fn rotate_crash_log(log_dir: &Path) {
    let crash_log = log_dir.join(CRASH_LOG_FILE_NAME);
    let Ok(metadata) = fs::metadata(&crash_log) else {
        return;
    };
    if metadata.len() < MAX_CRASH_LOG_BYTES {
        return;
    }

    let previous_crash_log = log_dir.join(PREVIOUS_CRASH_LOG_FILE_NAME);
    let _ = fs::remove_file(&previous_crash_log);
    let _ = fs::rename(crash_log, previous_crash_log);
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
    use std::{
        fs, io,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::{
        CRASH_LOG_FILE_NAME, MAX_CRASH_LOG_BYTES, PREVIOUS_CRASH_LOG_FILE_NAME, prune_startup_logs,
        rotate_crash_log, startup_log_timestamp, write_fatal_error_record,
    };

    static NEXT_TEST_DIR: AtomicU64 = AtomicU64::new(0);

    fn test_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "germinal-{name}-{}-{}",
            std::process::id(),
            NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn fatal_errors_are_written_to_an_explicit_crash_log() {
        let test_dir = test_dir("crash-log-test");
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

    #[test]
    fn startup_log_names_are_matched_exactly() {
        assert_eq!(startup_log_timestamp("germinal-123.log"), Some(123));
        assert_eq!(startup_log_timestamp("germinal-crash.log"), None);
        assert_eq!(startup_log_timestamp("germinal-+123.log"), None);
        assert_eq!(startup_log_timestamp("germinal-123.log.old"), None);
        assert_eq!(startup_log_timestamp("other-123.log"), None);
    }

    #[test]
    fn startup_log_pruning_keeps_newest_logs_and_unrelated_files() {
        let test_dir = test_dir("startup-log-prune-test");
        fs::create_dir_all(&test_dir).expect("test directory should be created");
        for timestamp in 1..=5 {
            fs::write(
                test_dir.join(format!("germinal-{timestamp}.log")),
                timestamp.to_string(),
            )
            .expect("startup log should be written");
        }
        fs::write(test_dir.join(CRASH_LOG_FILE_NAME), "crash")
            .expect("crash log should be written");
        fs::write(test_dir.join("notes.log"), "notes").expect("unrelated log should be written");

        prune_startup_logs(&test_dir, 2);

        assert!(!test_dir.join("germinal-1.log").exists());
        assert!(!test_dir.join("germinal-3.log").exists());
        assert!(test_dir.join("germinal-4.log").exists());
        assert!(test_dir.join("germinal-5.log").exists());
        assert!(test_dir.join(CRASH_LOG_FILE_NAME).exists());
        assert!(test_dir.join("notes.log").exists());
        fs::remove_dir_all(test_dir).expect("test directory should be removed");
    }

    #[test]
    fn oversized_crash_log_is_rotated_once() {
        let test_dir = test_dir("crash-log-rotate-test");
        fs::create_dir_all(&test_dir).expect("test directory should be created");
        let crash_log = test_dir.join(CRASH_LOG_FILE_NAME);
        fs::write(&crash_log, vec![0_u8; MAX_CRASH_LOG_BYTES as usize])
            .expect("crash log should be written");

        rotate_crash_log(&test_dir);

        assert!(!crash_log.exists());
        assert_eq!(
            fs::metadata(test_dir.join(PREVIOUS_CRASH_LOG_FILE_NAME))
                .expect("rotated crash log should exist")
                .len(),
            MAX_CRASH_LOG_BYTES
        );
        fs::remove_dir_all(test_dir).expect("test directory should be removed");
    }
}
