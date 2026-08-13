use std::{
	cell::{Cell, RefCell},
	collections::HashMap,
};

use germinal_domain::{
	gshell::vo::gshell_id::GShellId,
	workspace::{
		entity::{pane_tree::PaneTree, workspace::Workspace},
		vo::{pane_id::PaneId, pane_split_direction::PaneSplitDirection},
	},
};
use germinal_ports::{
	pty_host::window_size::TerminalWindowSize,
	rendering::{
		render_target_id::RenderTargetId, workspace_layout::RenderSurfacePlacement,
	},
	repository::IRepository,
	service::workspace_service::{IWorkspaceService, WorkspaceServiceError},
};

#[derive(kudi::DepInj)]
#[target(WorkspaceService)]
pub struct WorkspaceServiceState {
	persistence_workspace_id: Cell<Option<u64>>,
	workspace:                RefCell<Workspace>,
	pane_bindings:            RefCell<HashMap<PaneId, GShellId>>,
	next_gshell_id:           Cell<u64>,
}

impl WorkspaceServiceState {
	pub fn new() -> Self { Self::with_workspace(Workspace::main()) }

	pub fn with_workspace(workspace: Workspace) -> Self {
		let state = Self {
			persistence_workspace_id: Cell::new(None),
			workspace:                RefCell::new(workspace),
			pane_bindings:            RefCell::new(HashMap::new()),
			next_gshell_id:           Cell::new(0),
		};
		state.rebind_visible_panes();
		state
	}

	pub fn focused_gshell(&self) -> GShellId {
		let focused_pane = self.workspace.borrow().focused_pane();
		*self
			.pane_bindings
			.borrow()
			.get(&focused_pane)
			.expect("focused workspace pane must have a gshell binding")
	}

	pub fn visible_gshells(&self) -> Vec<GShellId> {
		let workspace = self.workspace.borrow();
		let bindings = self.pane_bindings.borrow();
		workspace
			.active_tab()
			.pane_tree()
			.pane_ids()
			.into_iter()
			.filter_map(|pane_id| bindings.get(&pane_id).copied())
			.collect()
	}

	pub fn render_layout(&self, window_size: TerminalWindowSize) -> Vec<RenderSurfacePlacement> {
		let workspace = self.workspace.borrow();
		let bindings = self.pane_bindings.borrow();
		let mut placements = Vec::with_capacity(workspace.active_tab().pane_count());
		collect_render_placements(
			workspace.active_tab().pane_tree(),
			&bindings,
			PixelRect::new(0, 0, window_size.width_px(), window_size.height_px()),
			&mut placements,
		);
		placements
	}

	pub fn workspace(&self) -> Workspace { self.workspace.borrow().clone() }

	fn persistence_workspace_id(&self) -> Option<u64> { self.persistence_workspace_id.get() }

	fn bind_workspace(&self, persistence_id: u64, workspace: Workspace) {
		self.persistence_workspace_id.set(Some(persistence_id));
		*self.workspace.borrow_mut() = workspace;
		self.rebind_visible_panes();
	}

	fn rebind_visible_panes(&self) {
		let pane_ids = self.workspace.borrow().active_tab().pane_tree().pane_ids();
		let mut bindings = self.pane_bindings.borrow_mut();
		bindings.retain(|pane_id, _| pane_ids.contains(pane_id));

		for pane_id in pane_ids {
			bindings.entry(pane_id).or_insert_with(|| {
				let gshell_id = GShellId::new(self.next_gshell_id.get());
				self.next_gshell_id.set(gshell_id.value() + 1);
				gshell_id
			});
		}
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PixelRect {
	x:      u32,
	y:      u32,
	width:  u32,
	height: u32,
}

impl PixelRect {
	const fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
		Self { x, y, width, height }
	}
}

fn collect_render_placements(
	tree: &PaneTree,
	bindings: &HashMap<PaneId, GShellId>,
	bounds: PixelRect,
	placements: &mut Vec<RenderSurfacePlacement>,
) {
	match tree {
		PaneTree::Pane(pane_id) => {
			let Some(gshell_id) = bindings.get(pane_id).copied() else {
				return;
			};
			placements.push(RenderSurfacePlacement::new(
				RenderTargetId::new(gshell_id.value()),
				bounds.x,
				bounds.y,
				bounds.width,
				bounds.height,
			));
		}
		PaneTree::Split { direction, first, second } => {
			let (first_bounds, second_bounds) = split_bounds(bounds, *direction);
			collect_render_placements(first, bindings, first_bounds, placements);
			collect_render_placements(second, bindings, second_bounds, placements);
		}
	}
}

fn split_bounds(bounds: PixelRect, direction: PaneSplitDirection) -> (PixelRect, PixelRect) {
	match direction {
		PaneSplitDirection::Horizontal => {
			let first_width = bounds.width / 2;
			let second_width = bounds.width.saturating_sub(first_width);
			(
				PixelRect::new(bounds.x, bounds.y, first_width, bounds.height),
				PixelRect::new(
					bounds.x.saturating_add(first_width),
					bounds.y,
					second_width,
					bounds.height,
				),
			)
		}
		PaneSplitDirection::Vertical => {
			let first_height = bounds.height / 2;
			let second_height = bounds.height.saturating_sub(first_height);
			(
				PixelRect::new(bounds.x, bounds.y, bounds.width, first_height),
				PixelRect::new(
					bounds.x,
					bounds.y.saturating_add(first_height),
					bounds.width,
					second_height,
				),
			)
		}
	}
}

impl<Deps> IWorkspaceService for WorkspaceService<Deps>
where Deps: AsRef<WorkspaceServiceState> + IRepository<Id = u64, Aggregate = Workspace>
{
	fn focused_gshell(&self) -> GShellId {
		<Deps as AsRef<WorkspaceServiceState>>::as_ref(self.prj_ref()).focused_gshell()
	}

	fn visible_gshells(&self) -> Vec<GShellId> {
		<Deps as AsRef<WorkspaceServiceState>>::as_ref(self.prj_ref()).visible_gshells()
	}

	fn workspace_render_layout(
		&self,
		window_size: TerminalWindowSize,
	) -> Vec<RenderSurfacePlacement> {
		<Deps as AsRef<WorkspaceServiceState>>::as_ref(self.prj_ref()).render_layout(window_size)
	}

	fn restore_workspace(&self) -> Result<(), WorkspaceServiceError> {
		let state = <Deps as AsRef<WorkspaceServiceState>>::as_ref(self.prj_ref());
		let repository = self.prj_ref();

		if let Some((persistence_id, workspace)) = repository
			.list()
			.map_err(|source| WorkspaceServiceError::Repository { source })?
			.into_iter()
			.next()
		{
			state.bind_workspace(persistence_id, workspace);
			return Ok(());
		}

		let workspace = state.workspace();
		let persistence_id = repository
			.insert(workspace.clone())
			.map_err(|source| WorkspaceServiceError::Repository { source })?;
		state.bind_workspace(persistence_id, workspace);
		Ok(())
	}

	fn persist_workspace(&self) -> Result<(), WorkspaceServiceError> {
		let state = <Deps as AsRef<WorkspaceServiceState>>::as_ref(self.prj_ref());
		let persistence_id =
			state.persistence_workspace_id().ok_or(WorkspaceServiceError::PersistenceIdNotInitialized)?;

		self
			.prj_ref()
			.update(persistence_id, state.workspace())
			.map_err(|source| WorkspaceServiceError::Repository { source })
	}
}

#[cfg(test)]
mod tests {
	use germinal_domain::workspace::entity::workspace::Workspace;
	use germinal_ports::pty_host::window_size::TerminalWindowSize;

	use super::WorkspaceServiceState;

	#[test]
	fn state_defaults_to_single_pane() {
		let state = WorkspaceServiceState::new();

		assert_eq!(state.visible_gshells().len(), 1);
	}

	#[test]
	fn state_binds_two_visible_panes_to_distinct_gshells() {
		let state = WorkspaceServiceState::with_workspace(Workspace::two_pane());

		let gshells = state.visible_gshells();
		assert_eq!(gshells.len(), 2);
		assert_ne!(gshells[0], gshells[1]);
		assert_eq!(state.focused_gshell(), gshells[1]);
	}

	#[test]
	fn horizontal_split_covers_odd_window_width_without_overlap() {
		let state = WorkspaceServiceState::with_workspace(Workspace::two_pane());

		let placements = state.render_layout(TerminalWindowSize::new(101, 40));

		assert_eq!(placements.len(), 2);
		assert_eq!(placements[0].x_px, 0);
		assert_eq!(placements[0].width_px, 50);
		assert_eq!(placements[1].x_px, 50);
		assert_eq!(placements[1].width_px, 51);
		assert_eq!(placements[0].height_px, 40);
		assert_eq!(placements[1].height_px, 40);
	}
}
