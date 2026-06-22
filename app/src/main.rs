mod app;

use germinal_ports::event::runtime_event::RuntimeEvent;
use winit::event_loop::EventLoop;

fn main() {
	let event_loop = match EventLoop::<RuntimeEvent>::with_user_event().build() {
		Ok(event_loop) => event_loop,
		Err(error) => {
			eprintln!("failed to create Germinal event loop: {error}");
			return;
		}
	};

	let mut app = match app::App::new(event_loop.create_proxy()) {
		Ok(app) => app,
		Err(error) => {
			eprintln!("failed to create Germinal app: {error}");
			return;
		}
	};

	if let Err(error) = app.run(event_loop) {
		eprintln!("failed to run Germinal: {error}");
	}
}
