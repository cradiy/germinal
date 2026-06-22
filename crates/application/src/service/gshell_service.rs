use std::{
	cell::RefCell,
	collections::HashMap,
	sync::{Arc, atomic::AtomicBool, mpsc::Sender},
};

use germinal_domain::{
	gshell::{
		entity::{gshell::GShell, gshell_mode::GShellMode},
		vo::gshell_id::GShellId,
	},
	pty_host::{entity::pty_host::PtyHost, pty_host_id::PtyHostId, terminal_size::TerminalGridSize},
};
use germinal_ports::{
	event::{
		gshell_input::{GShellInput, GShellInputEvent},
		runtime_event_dispatcher::RuntimeEventDispatcher,
		window_input_event::WindowInputEvent,
	},
	pty_host::terminal_size::TerminalPtySize,
	rendering::surface_snapshot::RenderSurfaceSnapshot,
	repository::IRepository,
	service::{
		gnative_service::IGNativeService, gshell_service::IGShellService,
		pty_host_service::IPtyHostRuntimeRepositoryProvider, pty_service::IPtyService,
	},
};

use crate::service::{gnative_service::GNativeServiceState, pty_service::PtyServiceState};

#[derive(kudi::DepInj)]
#[target(GShellService)]
pub struct GShellServiceState {
	pty_service_state:     PtyServiceState,
	gnative_service_state: GNativeServiceState,
	shells:                RefCell<HashMap<GShellId, GShell>>,
}

impl GShellServiceState {
	pub fn new() -> Self {
		Self {
			pty_service_state:     PtyServiceState::new(),
			gnative_service_state: GNativeServiceState::new(),
			shells:                RefCell::new(HashMap::new()),
		}
	}

	pub fn pty_service_state(&self) -> &PtyServiceState { &self.pty_service_state }

	pub fn gnative_service_state(&self) -> &GNativeServiceState { &self.gnative_service_state }

	fn mode_of(&self, gshell_id: GShellId) -> GShellMode {
		self.shells.borrow().get(&gshell_id).map(GShell::mode).unwrap_or(GShellMode::Pty)
	}

	fn pty_host_id_of(&self, gshell_id: GShellId) -> Option<PtyHostId> {
		self.shells.borrow().get(&gshell_id).map(GShell::pty_host_id)
	}
}

impl Default for GShellServiceState {
	fn default() -> Self { Self::new() }
}

impl<Deps> IGShellService for GShellService<Deps>
where Deps:
		AsRef<GShellServiceState> + IPtyHostRuntimeRepositoryProvider + IPtyService + IGNativeService
{
	fn ensure_gshell(
		&self,
		gshell_id: GShellId,
		proxy: RuntimeEventDispatcher,
		pty_size: TerminalPtySize,
		term_size: TerminalGridSize,
		surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
		snapshot_wake_pending: Arc<AtomicBool>,
	) {
		let state: &GShellServiceState = self.prj_ref().as_ref();
		let pty_host_id = if let Some(pty_host_id) = state.pty_host_id_of(gshell_id) {
			pty_host_id
		} else {
			let Ok(pty_host_raw_id) =
				self.prj_ref().pty_host_runtime_repository().insert(PtyHost::new(term_size))
			else {
				return;
			};
			let pty_host_id = PtyHostId::new(pty_host_raw_id);
			state.shells.borrow_mut().insert(gshell_id, GShell::new(pty_host_id));
			pty_host_id
		};

		sync_pty_host_size(self.prj_ref().pty_host_runtime_repository(), pty_host_id, term_size);
		self.prj_ref().ensure_gshell_pty(
			gshell_id,
			pty_host_id,
			proxy,
			pty_size,
			term_size,
			surface_snapshot_tx,
			snapshot_wake_pending,
		);
	}

	fn route_input_to_gshell(&self, input: GShellInput) {
		let state: &GShellServiceState = self.prj_ref().as_ref();

		match state.mode_of(input.gshell_id) {
			GShellMode::Pty => {
				let Some(pty_host_id) = state.pty_host_id_of(input.gshell_id) else {
					return;
				};
				self.prj_ref().send_pty_host_input(pty_host_id, input.event);
			}
			GShellMode::GNative => {
				if matches!(
					input.event,
					GShellInputEvent::Bytes(_)
						| GShellInputEvent::Paste(_)
						| GShellInputEvent::Window(WindowInputEvent::Key { .. })
						| GShellInputEvent::Window(WindowInputEvent::Ime(_))
						| GShellInputEvent::Window(WindowInputEvent::Paste(_))
				) {
					self.prj_ref().ensure_gshell_gnative(input.gshell_id);
				}
			}
		}
	}

	fn resize_gshell(
		&self,
		gshell_id: GShellId,
		pty_size: TerminalPtySize,
		term_size: TerminalGridSize,
	) {
		let state: &GShellServiceState = self.prj_ref().as_ref();

		match state.mode_of(gshell_id) {
			GShellMode::Pty => {
				let Some(pty_host_id) = state.pty_host_id_of(gshell_id) else {
					return;
				};
				sync_pty_host_size(self.prj_ref().pty_host_runtime_repository(), pty_host_id, term_size);
				self.prj_ref().resize_pty_host(pty_host_id, pty_size, term_size);
			}
			GShellMode::GNative => self.prj_ref().ensure_gshell_gnative(gshell_id),
		}
	}
}

fn sync_pty_host_size<Repo>(
	repository: &Repo,
	pty_host_id: PtyHostId,
	grid_size: TerminalGridSize,
) where
	Repo: germinal_ports::repository::IRepository<Id = u64, Aggregate = PtyHost>,
{
	let Ok(Some(mut pty_host)) = repository.get(pty_host_id.value()) else {
		return;
	};

	if !pty_host.resize(grid_size) {
		return;
	}

	let _ = repository.update(pty_host_id.value(), pty_host);
}
