use std::sync::Arc;

use crate::event::runtime_event::RuntimeEvent;

#[derive(Clone)]
pub struct RuntimeEventDispatcher {
	dispatch: Arc<dyn Fn(RuntimeEvent) -> Result<(), String> + Send + Sync>,
}

impl RuntimeEventDispatcher {
	pub fn new<F>(dispatch: F) -> Self
	where F: Fn(RuntimeEvent) -> Result<(), String> + Send + Sync + 'static {
		Self { dispatch: Arc::new(dispatch) }
	}

	pub fn dispatch(&self, event: RuntimeEvent) -> Result<(), String> { (self.dispatch)(event) }
}
