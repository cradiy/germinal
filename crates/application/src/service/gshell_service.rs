use std::{
	cell::{Cell, RefCell},
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
	event::gshell_input::GShellInput,
	pty_host::terminal_size::TerminalPtySize,
	rendering::surface_snapshot::RenderSurfaceSnapshot,
	service::{
		gnative_service::IGNativeService, gshell_service::IGShellService, pty_service::IPtyService,
	},
};

use crate::service::{gnative_service::GNativeServiceState, pty_service::PtyServiceState};

#[derive(kudi::DepInj)]
#[target(GShellService)]
pub struct GShellServiceState {
	pty_service_state:     PtyServiceState,
	gnative_service_state: GNativeServiceState,
	shells:                RefCell<HashMap<GShellId, GShell>>,
	pty_hosts:             RefCell<HashMap<PtyHostId, PtyHost>>,
	next_pty_host_raw_id:  Cell<u64>,
}

impl GShellServiceState {
	pub fn new() -> Self {
		Self {
			pty_service_state:     PtyServiceState::new(),
			gnative_service_state: GNativeServiceState::new(),
			shells:                RefCell::new(HashMap::new()),
			pty_hosts:             RefCell::new(HashMap::new()),
			next_pty_host_raw_id:  Cell::new(0),
		}
	}

	pub fn pty_service_state(&self) -> &PtyServiceState { &self.pty_service_state }

	pub fn gnative_service_state(&self) -> &GNativeServiceState { &self.gnative_service_state }

	pub fn begin_gnative_mode(&self, gshell_id: GShellId) {
		let mut shells = self.shells.borrow_mut();
		let Some(gshell) = shells.get_mut(&gshell_id) else {
			return;
		};
		gshell.begin_gnative();
	}

	pub fn enter_gnative_mode(&self, gshell_id: GShellId) {
		let mut shells = self.shells.borrow_mut();
		let Some(gshell) = shells.get_mut(&gshell_id) else {
			return;
		};
		gshell.enter_gnative();
	}

	pub fn exit_gnative_mode(&self, gshell_id: GShellId) {
		let mut shells = self.shells.borrow_mut();
		let Some(gshell) = shells.get_mut(&gshell_id) else {
			return;
		};
		gshell.exit_gnative();
	}

	fn mode_of(&self, gshell_id: GShellId) -> GShellMode {
		self.shells.borrow().get(&gshell_id).map(GShell::mode).unwrap_or(GShellMode::Pty)
	}

	fn pty_host_id_of(&self, gshell_id: GShellId) -> Option<PtyHostId> {
		self.shells.borrow().get(&gshell_id).map(GShell::pty_host_id)
	}

	fn create_pty_host(&self, gshell_id: GShellId, grid_size: TerminalGridSize) -> PtyHostId {
		let pty_host_id = PtyHostId::new(self.next_pty_host_raw_id.get());
		self.next_pty_host_raw_id.set(pty_host_id.value() + 1);
		self.pty_hosts.borrow_mut().insert(pty_host_id, PtyHost::new(grid_size));
		self.shells.borrow_mut().insert(gshell_id, GShell::new(pty_host_id));
		pty_host_id
	}

	fn sync_pty_host_size(&self, pty_host_id: PtyHostId, grid_size: TerminalGridSize) {
		let mut pty_hosts = self.pty_hosts.borrow_mut();
		let Some(pty_host) = pty_hosts.get_mut(&pty_host_id) else {
			return;
		};
		let _ = pty_host.resize(grid_size);
	}

	fn remove_gshell(&self, gshell_id: GShellId) -> Option<PtyHostId> {
		let pty_host_id = self.shells.borrow_mut().remove(&gshell_id)?.pty_host_id();
		self.pty_hosts.borrow_mut().remove(&pty_host_id);
		Some(pty_host_id)
	}
}

impl Default for GShellServiceState {
	fn default() -> Self { Self::new() }
}

impl<Deps> IGShellService for GShellService<Deps>
where Deps: AsRef<GShellServiceState> + IPtyService + IGNativeService
{
	fn ensure_gshell(
		&self,
		gshell_id: GShellId,
		pty_size: TerminalPtySize,
		term_size: TerminalGridSize,
		surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
		snapshot_wake_pending: Arc<AtomicBool>,
	) {
		let state = <Deps as AsRef<GShellServiceState>>::as_ref(self.prj_ref());
		let pty_host_id = if let Some(pty_host_id) = state.pty_host_id_of(gshell_id) {
			pty_host_id
		} else {
			state.create_pty_host(gshell_id, term_size)
		};

		state.sync_pty_host_size(pty_host_id, term_size);
		self.prj_ref().ensure_gshell_pty(
			gshell_id,
			pty_host_id,
			pty_size,
			term_size,
			surface_snapshot_tx,
			snapshot_wake_pending,
		);
	}

	fn begin_gnative_mode(&self, gshell_id: GShellId) {
		let state = <Deps as AsRef<GShellServiceState>>::as_ref(self.prj_ref());
		state.begin_gnative_mode(gshell_id);
	}

	fn enter_gnative_mode(&self, gshell_id: GShellId) {
		let state = <Deps as AsRef<GShellServiceState>>::as_ref(self.prj_ref());
		state.enter_gnative_mode(gshell_id);
	}

	fn exit_gnative_mode(&self, gshell_id: GShellId) {
		let state = <Deps as AsRef<GShellServiceState>>::as_ref(self.prj_ref());
		state.exit_gnative_mode(gshell_id);
	}

	fn remove_gshell(&self, gshell_id: GShellId) {
		let state = <Deps as AsRef<GShellServiceState>>::as_ref(self.prj_ref());
		let Some(pty_host_id) = state.remove_gshell(gshell_id) else {
			return;
		};

		self.prj_ref().remove_pty_host(pty_host_id);
		self.prj_ref().exit_gnative_session(gshell_id);
	}

	fn route_input_to_gshell(&self, input: GShellInput) {
		let state = <Deps as AsRef<GShellServiceState>>::as_ref(self.prj_ref());

		match state.mode_of(input.gshell_id) {
			GShellMode::Pty | GShellMode::GNativeConnecting => {
				let Some(pty_host_id) = state.pty_host_id_of(input.gshell_id) else {
					return;
				};
				self.prj_ref().send_pty_host_input(pty_host_id, input.event);
			}
			GShellMode::GNative => self.prj_ref().route_gnative_input(input),
		}
	}

	fn resize_gshell(
		&self,
		gshell_id: GShellId,
		pty_size: TerminalPtySize,
		term_size: TerminalGridSize,
	) {
		let state = <Deps as AsRef<GShellServiceState>>::as_ref(self.prj_ref());

		match state.mode_of(gshell_id) {
			GShellMode::Pty | GShellMode::GNativeConnecting => {
				let Some(pty_host_id) = state.pty_host_id_of(gshell_id) else {
					return;
				};
				state.sync_pty_host_size(pty_host_id, term_size);
				self.prj_ref().resize_pty_host(pty_host_id, pty_size, term_size);
			}
			GShellMode::GNative => self.prj_ref().resize_gnative_session(gshell_id, term_size),
		}
	}
}

#[cfg(test)]
mod tests {
	use germinal_domain::{
		gshell::{entity::gshell_mode::GShellMode, vo::gshell_id::GShellId},
		pty_host::terminal_size::TerminalGridSize,
	};

	use super::GShellServiceState;

	#[test]
	fn state_switches_between_pty_and_gnative_modes() {
		let state = GShellServiceState::new();
		let gshell_id = GShellId::new(7);
		state.create_pty_host(gshell_id, TerminalGridSize::new(80, 24));

		assert_eq!(state.mode_of(gshell_id), GShellMode::Pty);

		state.begin_gnative_mode(gshell_id);
		assert_eq!(state.mode_of(gshell_id), GShellMode::GNativeConnecting);

		state.enter_gnative_mode(gshell_id);
		assert_eq!(state.mode_of(gshell_id), GShellMode::GNative);

		state.exit_gnative_mode(gshell_id);
		assert_eq!(state.mode_of(gshell_id), GShellMode::Pty);
	}

	#[test]
	fn state_removes_a_gshell_and_its_pty_host() {
		let state = GShellServiceState::new();
		let gshell_id = GShellId::new(7);
		let pty_host_id = state.create_pty_host(gshell_id, TerminalGridSize::new(80, 24));

		assert_eq!(state.remove_gshell(gshell_id), Some(pty_host_id));
		assert_eq!(state.pty_host_id_of(gshell_id), None);
		assert!(!state.pty_hosts.borrow().contains_key(&pty_host_id));
	}
}
