use thiserror::Error;

use crate::event::runtime_event::RuntimeEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum RuntimeEventDispatchError {
	#[error("runtime event dispatcher is closed")]
	Closed,
}

pub trait IRuntimeEventDispatcher: Clone + Send + 'static {
	fn dispatch(&self, event: RuntimeEvent) -> Result<(), RuntimeEventDispatchError>;
}

pub trait IRuntimeEventDispatcherProvider {
	type RuntimeEventDispatcher: IRuntimeEventDispatcher;

	fn runtime_event_dispatcher(&self) -> &Self::RuntimeEventDispatcher;
}
