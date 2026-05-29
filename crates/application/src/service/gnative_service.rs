use germinal_domain::workspace::pane_id::PaneId;
use germinal_ports::service::{gnative_service::IGNativeService, worker_service::IWorkerService};

#[derive(kudi::DepInj)]
#[target(GNativeService)]
pub struct GNativeServiceState;

impl GNativeServiceState {
	pub fn new() -> Self { Self }
}

impl Default for GNativeServiceState {
	fn default() -> Self { Self::new() }
}

impl<Deps> IGNativeService for GNativeService<Deps>
where Deps: AsRef<GNativeServiceState> + IWorkerService
{
	fn ensure_pane_gnative(&self, _pane_id: PaneId) { self.prj_ref().start_worker_pool(); }
}
