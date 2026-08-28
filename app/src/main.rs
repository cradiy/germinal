mod app;

use clap::Parser;
use eros::Context;
use germinal_ports::event::runtime_event::RuntimeEvent;
use std::process::ExitCode;
use tracing::info;
use winit::event_loop::EventLoop;

#[derive(Debug, Parser)]
#[command(version, about = "A GPU-accelerated terminal emulator")]
struct Cli;

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
    Cli::parse();
    app::prepare_crash_reporting();

    let (config, paths) = app::load_or_create_config().context("failed to load Germinal config")?;
    app::init_logging(&config.logging, &paths).context("failed to initialize Germinal logging")?;

    let event_loop = EventLoop::<RuntimeEvent>::with_user_event()
        .build()
        .context("failed to create Germinal event loop")?;
    let mut app = app::App::new(event_loop.create_proxy(), config)
        .context("failed to create Germinal app")?;

    info!("starting Germinal");
    app.run(event_loop).context("failed to run Germinal")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_an_empty_command_line() {
        Cli::try_parse_from(["germinal"]).expect("an empty command line should start Germinal");
    }

    #[test]
    fn reports_the_package_version() {
        let error = Cli::try_parse_from(["germinal", "--version"])
            .expect_err("--version should print the package version and exit");

        assert_eq!(error.exit_code(), 0);
        assert_eq!(
            error.to_string(),
            format!("germinal {}\n", env!("CARGO_PKG_VERSION"))
        );
    }
}
