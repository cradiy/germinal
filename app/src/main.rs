#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;

use clap::{Parser, builder::TypedValueParser};
use eros::Context;
use germinal_ports::event::runtime_event::RuntimeEvent;
use std::{path::PathBuf, process::ExitCode};
use tracing::info;
use winit::event_loop::EventLoop;

#[derive(Debug, Parser)]
#[command(version, about = "A GPU-accelerated terminal emulator")]
struct Cli {
    /// Start the terminal in this directory (overrides the configured working directory).
    #[arg(value_name = "DIRECTORY", value_parser = clap::builder::PathBufValueParser::new().try_map(parse_directory))]
    directory: Option<PathBuf>,
}

fn parse_directory(path: PathBuf) -> Result<PathBuf, String> {
    let resolved = path
        .canonicalize()
        .map_err(|error| format!("cannot open directory '{}': {error}", path.display()))?;
    if !resolved.is_dir() {
        return Err(format!("not a directory: '{}'", path.display()));
    }
    Ok(resolved)
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            app::report_fatal_error(&error);
            eprintln!("Germinal failed: {error:?}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> eros::Result<()> {
    let cli = Cli::parse();
    app::prepare_crash_reporting();

    let (mut config, paths, config_warning) =
        app::load_or_create_config().context("failed to load Germinal config")?;
    if let Some(warning) = &config_warning {
        eprintln!("Germinal configuration warning: {warning}");
    }
    if let Some(directory) = cli.directory {
        config.terminal.working_directory = Some(directory);
    }
    app::init_logging(&config.logging, &paths).context("failed to initialize Germinal logging")?;
    if let Some(warning) = config_warning {
        tracing::warn!("{warning}");
    }

    let event_loop = EventLoop::<RuntimeEvent>::with_user_event()
        .build()
        .context("failed to create Germinal event loop")?;
    let mut app = app::App::new(event_loop.create_proxy(), config)
        .context("failed to create Germinal app")?;
    app.restore_ui_state(paths.ui_state_file());

    info!("starting Germinal");
    app.run(event_loop).context("failed to run Germinal")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_an_empty_command_line() {
        let cli = Cli::try_parse_from(["germinal"]).unwrap();
        assert!(cli.directory.is_none());
        let cli = Cli::try_parse_from(["germinal", "."]).unwrap();
        assert_eq!(
            cli.directory,
            Some(std::env::current_dir().unwrap().canonicalize().unwrap())
        );
    }

    #[test]
    fn rejects_files_and_missing_directories_before_startup() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        for path in [
            root.join("Cargo.toml"),
            root.join("Cargo.toml/missing-directory"),
        ] {
            let error = Cli::try_parse_from([std::ffi::OsStr::new("germinal"), path.as_os_str()])
                .unwrap_err();
            assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
        }
    }
}
