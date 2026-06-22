use germinal_domain::workspace::pane_id::PaneId;
use germinal_ports::event::runtime_event_dispatcher::RuntimeEventDispatcher;

#[derive(kudi::DepInj)]
#[target(WorkspaceService)]
pub struct WorkspaceServiceState {
	focused_pane:        PaneId,
	runtime_event_proxy: RuntimeEventDispatcher,
}

impl WorkspaceServiceState {
	pub fn new(runtime_event_proxy: RuntimeEventDispatcher) -> Self {
		Self { focused_pane: PaneId::new(0), runtime_event_proxy }
	}

	pub fn focused_pane(&self) -> PaneId { self.focused_pane }

	pub fn runtime_event_proxy(&self) -> RuntimeEventDispatcher { self.runtime_event_proxy.clone() }
}
