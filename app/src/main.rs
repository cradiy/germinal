mod app;

use eyre::WrapErr;
use germinal_ports::event::runtime_event::RuntimeEvent;
use tracing::info;
use winit::event_loop::EventLoop;

fn main() -> eyre::Result<()> {
	let (config, paths) = app::load_or_create_config().wrap_err("failed to load Germinal config")?;
	app::init_logging(&config.logging, &paths).wrap_err("failed to initialize Germinal logging")?;

	let event_loop = EventLoop::<RuntimeEvent>::with_user_event()
		.build()
		.wrap_err("failed to create Germinal event loop")?;
	let mut app = app::App::new(event_loop.create_proxy(), config, paths)
		.wrap_err("failed to create Germinal app")?;

	info!("starting Germinal");
	app.run(event_loop).wrap_err("failed to run Germinal")
}
