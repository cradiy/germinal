use germinal_domain::workspace::pane_id::PaneId;
use germinal_ports::event::{
	runtime_event::RuntimeEvent, runtime_event_dispatcher::RuntimeEventDispatcher,
};
use winit::event_loop::EventLoopProxy;

#[derive(kudi::DepInj)]
#[target(WorkspaceService)]
pub struct WorkspaceServiceState {
	focused_pane:        PaneId,
	runtime_event_proxy: RuntimeEventDispatcher,
}

impl WorkspaceServiceState {
	pub fn new(proxy: EventLoopProxy<RuntimeEvent>) -> Self {
		let runtime_event_proxy = RuntimeEventDispatcher::new(move |event| {
			proxy.send_event(event).map_err(|error| error.to_string())
		});

		Self { focused_pane: PaneId::new(0), runtime_event_proxy }
	}

	pub fn focused_pane(&self) -> PaneId { self.focused_pane }

	pub fn runtime_event_proxy(&self) -> RuntimeEventDispatcher { self.runtime_event_proxy.clone() }
}
