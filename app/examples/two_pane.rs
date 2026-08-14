use eros::Context;
use germinal::app;
use germinal_domain::workspace::entity::workspace::Workspace;
use germinal_ports::event::runtime_event::RuntimeEvent;
use tracing::info;
use winit::event_loop::EventLoop;

fn main() -> eros::Result<()> {
    let (config, paths) = app::load_or_create_config().context("failed to load Germinal config")?;
    app::init_logging(&config.logging, &paths).context("failed to initialize Germinal logging")?;

    let event_loop = EventLoop::<RuntimeEvent>::with_user_event()
        .build()
        .context("failed to create Germinal event loop")?;
    let mut app =
        app::App::new_with_workspace(event_loop.create_proxy(), config, Workspace::two_pane())
            .context("failed to create two-pane Germinal example")?;

    info!("starting Germinal two-pane example");
    app.run(event_loop)
        .context("failed to run Germinal two-pane example")?;
    Ok(())
}
