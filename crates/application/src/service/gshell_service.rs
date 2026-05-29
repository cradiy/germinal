use std::{
	cell::RefCell,
	collections::HashMap,
	sync::{Arc, atomic::AtomicBool, mpsc::Sender},
};

use germinal_domain::{
	pty_host::terminal_size::{TerminalGridSize, TerminalPtySize},
	workspace::pane_id::PaneId,
};
use germinal_ports::{
	event::{
		gshell_input::{GShellInput, GShellInputEvent},
		runtime_event_dispatcher::RuntimeEventDispatcher,
		window_input_event::WindowInputEvent,
	},
	rendering::surface_snapshot::RenderSurfaceSnapshot,
	service::{
		gnative_service::IGNativeService, gshell_service::IGShellService, pty_service::IPtyService,
	},
};

use crate::service::{gnative_service::GNativeServiceState, pty_service::PtyServiceState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GShellMode {
	Pty,
	GNative,
}

#[derive(Debug)]
pub struct GShell {
	mode: GShellMode,
}

impl GShell {
	pub fn new() -> Self { Self { mode: GShellMode::Pty } }

	pub fn mode(&self) -> GShellMode { self.mode }

	pub fn enter_gnative(&mut self) { self.mode = GShellMode::GNative; }
}

impl Default for GShell {
	fn default() -> Self { Self::new() }
}

#[derive(kudi::DepInj)]
#[target(GShellService)]
pub struct GShellServiceState {
	pty_service_state:     PtyServiceState,
	gnative_service_state: GNativeServiceState,
	shells:                RefCell<HashMap<PaneId, GShell>>,
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

	fn ensure_pane_gshell_state(&self, pane_id: PaneId) {
		self.shells.borrow_mut().entry(pane_id).or_insert_with(GShell::new);
	}

	fn mode_of(&self, pane_id: PaneId) -> GShellMode {
		self.shells.borrow().get(&pane_id).map(GShell::mode).unwrap_or(GShellMode::Pty)
	}
}

impl Default for GShellServiceState {
	fn default() -> Self { Self::new() }
}

impl<Deps> IGShellService for GShellService<Deps>
where Deps: AsRef<GShellServiceState> + IPtyService + IGNativeService
{
	fn ensure_pane_gshell(
		&self,
		pane_id: PaneId,
		proxy: RuntimeEventDispatcher,
		pty_size: TerminalPtySize,
		term_size: TerminalGridSize,
		surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
		snapshot_wake_pending: Arc<AtomicBool>,
	) {
		let state: &GShellServiceState = self.prj_ref().as_ref();
		state.ensure_pane_gshell_state(pane_id);
		self.prj_ref().ensure_pane_pty(
			pane_id,
			proxy,
			pty_size,
			term_size,
			surface_snapshot_tx,
			snapshot_wake_pending,
		);
	}

	fn route_input_to_gshell(&self, input: GShellInput) {
		let state: &GShellServiceState = self.prj_ref().as_ref();

		match state.mode_of(input.pane_id) {
			GShellMode::Pty => self.prj_ref().send_pane_pty_input(input),
			GShellMode::GNative => {
				if matches!(
					input.event,
					GShellInputEvent::Bytes(_)
						| GShellInputEvent::Paste(_)
						| GShellInputEvent::Window(WindowInputEvent::Key { .. })
						| GShellInputEvent::Window(WindowInputEvent::Ime(_))
						| GShellInputEvent::Window(WindowInputEvent::Paste(_))
				) {
					self.prj_ref().ensure_pane_gnative(input.pane_id);
				}
			}
		}
	}

	fn resize_pane_gshell(
		&self,
		pane_id: PaneId,
		pty_size: TerminalPtySize,
		term_size: TerminalGridSize,
	) {
		let state: &GShellServiceState = self.prj_ref().as_ref();

		match state.mode_of(pane_id) {
			GShellMode::Pty => self.prj_ref().resize_pane_pty(pane_id, pty_size, term_size),
			GShellMode::GNative => self.prj_ref().ensure_pane_gnative(pane_id),
		}
	}
}
