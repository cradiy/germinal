use crate::{error::BoxResult, event::runtime_event::RuntimeEvent};

pub trait IRuntimeEventDispatcher: Clone + Send + 'static {
	fn dispatch(&self, event: RuntimeEvent) -> BoxResult<()>;
}

pub trait IRuntimeEventDispatcherProvider {
	type RuntimeEventDispatcher: IRuntimeEventDispatcher;

	fn runtime_event_dispatcher(&self) -> &Self::RuntimeEventDispatcher;
}
