use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    env,
    path::Path,
};

use germinal_domain::{
    gshell::vo::gshell_id::GShellId,
    workspace::{
        entity::{pane_tree::PaneTree, workspace::Workspace},
        vo::{pane_id::PaneId, pane_split_direction::PaneSplitDirection, tab_id::TabId},
    },
};
use germinal_ports::{
    pty_host::window_size::TerminalWindowSize,
    rendering::{render_target_id::RenderTargetId, workspace_layout::RenderSurfacePlacement},
    repository::IRepository,
    service::workspace_service::{
        IWorkspaceService, WorkspaceGShellCloseOutcome, WorkspaceServiceError,
    },
};

#[derive(kudi::DepInj)]
#[target(WorkspaceService)]
pub struct WorkspaceServiceState {
    persistence_workspace_id: Cell<Option<u64>>,
    workspace: RefCell<Workspace>,
    pane_bindings: RefCell<HashMap<(TabId, PaneId), GShellId>>,
    gshell_titles: RefCell<HashMap<GShellId, String>>,
    default_tab_title: String,
    next_gshell_id: Cell<u64>,
}

impl WorkspaceServiceState {
    pub fn new() -> Self {
        Self::with_workspace(Workspace::main())
    }

    pub fn with_workspace(workspace: Workspace) -> Self {
        let state = Self {
            persistence_workspace_id: Cell::new(None),
            workspace: RefCell::new(workspace),
            pane_bindings: RefCell::new(HashMap::new()),
            gshell_titles: RefCell::new(HashMap::new()),
            default_tab_title: default_tab_title(),
            next_gshell_id: Cell::new(0),
        };
        state.rebind_all_panes();
        state
    }

    pub fn focused_gshell(&self) -> GShellId {
        let workspace = self.workspace.borrow();
        let key = (workspace.active_tab_id(), workspace.focused_pane());
        *self
            .pane_bindings
            .borrow()
            .get(&key)
            .expect("focused workspace pane must have a gshell binding")
    }

    pub fn focus_gshell(&self, gshell_id: GShellId) -> bool {
        let active_tab_id = self.workspace.borrow().active_tab_id();
        let pane_id =
            self.pane_bindings
                .borrow()
                .iter()
                .find_map(|((tab_id, pane_id), bound_gshell_id)| {
                    (*tab_id == active_tab_id && *bound_gshell_id == gshell_id).then_some(*pane_id)
                });
        let Some(pane_id) = pane_id else {
            return false;
        };

        self.workspace.borrow_mut().set_focused_pane(pane_id)
    }

    pub fn focus_next_gshell(&self) -> GShellId {
        let mut workspace = self.workspace.borrow_mut();
        let focused_pane = workspace.focus_next_pane();
        let key = (workspace.active_tab_id(), focused_pane);
        *self
            .pane_bindings
            .borrow()
            .get(&key)
            .expect("focused workspace pane must have a gshell binding")
    }

    pub fn focus_previous_gshell(&self) -> GShellId {
        let mut workspace = self.workspace.borrow_mut();
        let focused_pane = workspace.focus_previous_pane();
        let key = (workspace.active_tab_id(), focused_pane);
        *self
            .pane_bindings
            .borrow()
            .get(&key)
            .expect("focused workspace pane must have a gshell binding")
    }

    pub fn create_tab_gshell(&self) -> GShellId {
        let mut workspace = self.workspace.borrow_mut();
        let tab_id = workspace.create_tab();
        let pane_id = workspace.focused_pane();
        drop(workspace);

        let gshell_id = self.allocate_gshell_id();
        self.pane_bindings
            .borrow_mut()
            .insert((tab_id, pane_id), gshell_id);
        gshell_id
    }

    pub fn activate_next_tab(&self) -> GShellId {
        self.workspace.borrow_mut().activate_next_tab();
        self.focused_gshell()
    }

    pub fn activate_previous_tab(&self) -> GShellId {
        self.workspace.borrow_mut().activate_previous_tab();
        self.focused_gshell()
    }

    pub fn tab_count(&self) -> usize {
        self.workspace.borrow().tab_count()
    }

    pub fn active_tab_index(&self) -> usize {
        self.workspace.borrow().active_tab_index()
    }

    pub fn tab_titles(&self) -> Vec<String> {
        let titles = self.gshell_titles.borrow();
        self.tab_gshells()
            .into_iter()
            .map(|gshell_id| {
                titles
                    .get(&gshell_id)
                    .cloned()
                    .unwrap_or_else(|| self.default_tab_title.clone())
            })
            .collect()
    }

    pub fn tab_gshells(&self) -> Vec<GShellId> {
        let workspace = self.workspace.borrow();
        let bindings = self.pane_bindings.borrow();
        workspace
            .tabs()
            .iter()
            .map(|tab| {
                *bindings
                    .get(&(tab.tab_id(), tab.focused_pane()))
                    .expect("workspace tab focused pane must have a gshell binding")
            })
            .collect()
    }

    pub fn update_gshell_title(&self, gshell_id: GShellId, title: Option<String>) {
        let mut titles = self.gshell_titles.borrow_mut();
        if let Some(title) = title {
            titles.insert(gshell_id, title);
        } else {
            titles.remove(&gshell_id);
        }
    }

    pub fn split_focused_gshell(&self, direction: PaneSplitDirection) -> GShellId {
        let mut workspace = self.workspace.borrow_mut();
        let pane_id = workspace.split_focused_pane(direction);
        let tab_id = workspace.active_tab_id();
        drop(workspace);

        let gshell_id = self.allocate_gshell_id();
        self.pane_bindings
            .borrow_mut()
            .insert((tab_id, pane_id), gshell_id);
        gshell_id
    }

    pub fn swap_focused_gshell_with(&self, other: GShellId) -> bool {
        let active_tab_id = self.workspace.borrow().active_tab_id();
        let other_pane =
            self.pane_bindings
                .borrow()
                .iter()
                .find_map(|((tab_id, pane_id), gshell_id)| {
                    (*tab_id == active_tab_id && *gshell_id == other).then_some(*pane_id)
                });
        let Some(other_pane) = other_pane else {
            return false;
        };

        self.workspace
            .borrow_mut()
            .swap_focused_pane_with(other_pane)
    }

    pub fn close_gshell(&self, gshell_id: GShellId) -> Option<WorkspaceGShellCloseOutcome> {
        let (tab_id, pane_id) = self
            .pane_bindings
            .borrow()
            .iter()
            .find_map(|(key, bound_gshell_id)| (*bound_gshell_id == gshell_id).then_some(*key))?;

        let mut workspace = self.workspace.borrow_mut();
        let tab = workspace
            .tab(tab_id)
            .expect("bound pane must belong to a workspace tab");

        if tab.pane_count() == 1 && workspace.tab_count() == 1 {
            return Some(WorkspaceGShellCloseOutcome::CloseWorkspace);
        }

        let closed_keys = if tab.pane_count() == 1 {
            let removed = workspace
                .close_tab(tab_id)
                .expect("a tab can be closed while another tab remains");
            removed
                .pane_tree()
                .pane_ids()
                .into_iter()
                .map(|pane_id| (tab_id, pane_id))
                .collect::<Vec<_>>()
        } else {
            let closed = workspace.close_pane_in_tab(tab_id, pane_id);
            debug_assert!(closed, "bound pane must be removable from its tab");
            vec![(tab_id, pane_id)]
        };
        drop(workspace);

        let mut bindings = self.pane_bindings.borrow_mut();
        let closed_gshells = closed_keys
            .into_iter()
            .filter_map(|key| bindings.remove(&key))
            .collect::<Vec<_>>();
        drop(bindings);
        let mut titles = self.gshell_titles.borrow_mut();
        for closed_gshell in &closed_gshells {
            titles.remove(closed_gshell);
        }
        drop(titles);

        Some(WorkspaceGShellCloseOutcome::Closed {
            closed_gshells,
            focused_gshell: self.focused_gshell(),
        })
    }

    pub fn visible_gshells(&self) -> Vec<GShellId> {
        let workspace = self.workspace.borrow();
        let bindings = self.pane_bindings.borrow();
        workspace
            .active_tab()
            .pane_tree()
            .pane_ids()
            .into_iter()
            .filter_map(|pane_id| bindings.get(&(workspace.active_tab_id(), pane_id)).copied())
            .collect()
    }

    pub fn render_layout(&self, window_size: TerminalWindowSize) -> Vec<RenderSurfacePlacement> {
        let workspace = self.workspace.borrow();
        let bindings = self.pane_bindings.borrow();
        let mut placements = Vec::with_capacity(workspace.active_tab().pane_count());
        collect_render_placements(
            workspace.active_tab().pane_tree(),
            workspace.active_tab_id(),
            &bindings,
            PixelRect::new(0, 0, window_size.width_px(), window_size.height_px()),
            &mut placements,
        );
        placements
    }

    pub fn workspace(&self) -> Workspace {
        self.workspace.borrow().clone()
    }

    fn persistence_workspace_id(&self) -> Option<u64> {
        self.persistence_workspace_id.get()
    }

    fn bind_workspace(&self, persistence_id: u64, workspace: Workspace) {
        self.persistence_workspace_id.set(Some(persistence_id));
        *self.workspace.borrow_mut() = workspace;
        self.rebind_all_panes();
    }

    fn rebind_all_panes(&self) {
        let pane_keys = self
            .workspace
            .borrow()
            .tabs()
            .iter()
            .flat_map(|tab| {
                tab.pane_tree()
                    .pane_ids()
                    .into_iter()
                    .map(|pane_id| (tab.tab_id(), pane_id))
            })
            .collect::<Vec<_>>();
        let mut bindings = self.pane_bindings.borrow_mut();
        bindings.retain(|key, _| pane_keys.contains(key));

        for key in pane_keys {
            bindings
                .entry(key)
                .or_insert_with(|| self.allocate_gshell_id());
        }
    }

    fn allocate_gshell_id(&self) -> GShellId {
        let gshell_id = GShellId::new(self.next_gshell_id.get());
        self.next_gshell_id.set(gshell_id.value() + 1);
        gshell_id
    }
}

fn default_tab_title() -> String {
    let Ok(current_dir) = env::current_dir() else {
        return ".".to_string();
    };
    let home_dir = env::var_os("HOME").map(std::path::PathBuf::from);
    pretty_path(&current_dir, home_dir.as_deref())
}

fn pretty_path(path: &Path, home_dir: Option<&Path>) -> String {
    if let Some(home_dir) = home_dir
        && let Ok(relative) = path.strip_prefix(home_dir)
    {
        if relative.as_os_str().is_empty() {
            return "~".to_string();
        }
        return format!("~/{}", relative.display());
    }

    path.display().to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PixelRect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl PixelRect {
    const fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

fn collect_render_placements(
    tree: &PaneTree,
    tab_id: TabId,
    bindings: &HashMap<(TabId, PaneId), GShellId>,
    bounds: PixelRect,
    placements: &mut Vec<RenderSurfacePlacement>,
) {
    match tree {
        PaneTree::Pane(pane_id) => {
            let Some(gshell_id) = bindings.get(&(tab_id, *pane_id)).copied() else {
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
        PaneTree::Split {
            direction,
            first,
            second,
        } => {
            let (first_bounds, second_bounds) = split_bounds(bounds, *direction);
            collect_render_placements(first, tab_id, bindings, first_bounds, placements);
            collect_render_placements(second, tab_id, bindings, second_bounds, placements);
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
where
    Deps: AsRef<WorkspaceServiceState> + IRepository<Id = u64, Aggregate = Workspace>,
{
    fn focused_gshell(&self) -> GShellId {
        <Deps as AsRef<WorkspaceServiceState>>::as_ref(self.prj_ref()).focused_gshell()
    }

    fn focus_gshell(&self, gshell_id: GShellId) -> bool {
        <Deps as AsRef<WorkspaceServiceState>>::as_ref(self.prj_ref()).focus_gshell(gshell_id)
    }

    fn focus_next_gshell(&self) -> GShellId {
        <Deps as AsRef<WorkspaceServiceState>>::as_ref(self.prj_ref()).focus_next_gshell()
    }

    fn focus_previous_gshell(&self) -> GShellId {
        <Deps as AsRef<WorkspaceServiceState>>::as_ref(self.prj_ref()).focus_previous_gshell()
    }

    fn create_tab_gshell(&self) -> GShellId {
        <Deps as AsRef<WorkspaceServiceState>>::as_ref(self.prj_ref()).create_tab_gshell()
    }

    fn activate_next_tab(&self) -> GShellId {
        <Deps as AsRef<WorkspaceServiceState>>::as_ref(self.prj_ref()).activate_next_tab()
    }

    fn activate_previous_tab(&self) -> GShellId {
        <Deps as AsRef<WorkspaceServiceState>>::as_ref(self.prj_ref()).activate_previous_tab()
    }

    fn tab_count(&self) -> usize {
        <Deps as AsRef<WorkspaceServiceState>>::as_ref(self.prj_ref()).tab_count()
    }

    fn active_tab_index(&self) -> usize {
        <Deps as AsRef<WorkspaceServiceState>>::as_ref(self.prj_ref()).active_tab_index()
    }

    fn tab_titles(&self) -> Vec<String> {
        <Deps as AsRef<WorkspaceServiceState>>::as_ref(self.prj_ref()).tab_titles()
    }

    fn tab_gshells(&self) -> Vec<GShellId> {
        <Deps as AsRef<WorkspaceServiceState>>::as_ref(self.prj_ref()).tab_gshells()
    }

    fn update_gshell_title(&self, gshell_id: GShellId, title: Option<String>) {
        <Deps as AsRef<WorkspaceServiceState>>::as_ref(self.prj_ref())
            .update_gshell_title(gshell_id, title)
    }

    fn split_focused_gshell(&self, direction: PaneSplitDirection) -> GShellId {
        <Deps as AsRef<WorkspaceServiceState>>::as_ref(self.prj_ref())
            .split_focused_gshell(direction)
    }

    fn swap_focused_gshell_with(&self, other: GShellId) -> bool {
        <Deps as AsRef<WorkspaceServiceState>>::as_ref(self.prj_ref())
            .swap_focused_gshell_with(other)
    }

    fn close_gshell(&self, gshell_id: GShellId) -> Option<WorkspaceGShellCloseOutcome> {
        <Deps as AsRef<WorkspaceServiceState>>::as_ref(self.prj_ref()).close_gshell(gshell_id)
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
        let persistence_id = state
            .persistence_workspace_id()
            .ok_or(WorkspaceServiceError::PersistenceIdNotInitialized)?;

        self.prj_ref()
            .update(persistence_id, state.workspace())
            .map_err(|source| WorkspaceServiceError::Repository { source })
    }
}

#[cfg(test)]
mod tests {
    use germinal_domain::{
        gshell::vo::gshell_id::GShellId,
        workspace::{entity::workspace::Workspace, vo::pane_split_direction::PaneSplitDirection},
    };
    use germinal_ports::{
        pty_host::window_size::TerminalWindowSize,
        service::workspace_service::WorkspaceGShellCloseOutcome,
    };

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
        assert_eq!(state.focus_next_gshell(), gshells[0]);
        assert_eq!(state.focus_next_gshell(), gshells[1]);
        assert_eq!(state.focus_previous_gshell(), gshells[0]);
        assert_eq!(state.focus_previous_gshell(), gshells[1]);
    }

    #[test]
    fn state_focuses_a_visible_gshell_and_rejects_an_unknown_one() {
        let state = WorkspaceServiceState::with_workspace(Workspace::two_pane());
        let gshells = state.visible_gshells();

        assert!(state.focus_gshell(gshells[0]));
        assert_eq!(state.focused_gshell(), gshells[0]);
        assert!(!state.focus_gshell(GShellId::new(99)));
        assert_eq!(state.focused_gshell(), gshells[0]);
    }

    #[test]
    fn state_splits_the_focused_pane_and_binds_a_new_focused_gshell() {
        let state = WorkspaceServiceState::new();
        let original = state.focused_gshell();

        let created = state.split_focused_gshell(PaneSplitDirection::Vertical);

        assert_ne!(created, original);
        assert_eq!(state.visible_gshells(), vec![original, created]);
        assert_eq!(state.focused_gshell(), created);

        let placements = state.render_layout(TerminalWindowSize::new(80, 41));
        assert_eq!(placements.len(), 2);
        assert_eq!(placements[0].height_px, 20);
        assert_eq!(placements[1].y_px, 20);
        assert_eq!(placements[1].height_px, 21);
    }

    #[test]
    fn state_swaps_focused_gshell_position_and_keeps_its_focus() {
        let state = WorkspaceServiceState::with_workspace(Workspace::two_pane());
        let gshells = state.visible_gshells();

        assert!(state.swap_focused_gshell_with(gshells[0]));
        assert_eq!(state.visible_gshells(), vec![gshells[1], gshells[0]]);
        assert_eq!(state.focused_gshell(), gshells[1]);
        assert!(!state.swap_focused_gshell_with(GShellId::new(99)));
    }

    #[test]
    fn state_closes_a_visible_gshell_and_focuses_the_remaining_one() {
        let state = WorkspaceServiceState::with_workspace(Workspace::two_pane());
        let gshells = state.visible_gshells();

        assert_eq!(
            state.close_gshell(gshells[1]),
            Some(WorkspaceGShellCloseOutcome::Closed {
                closed_gshells: vec![gshells[1]],
                focused_gshell: gshells[0],
            })
        );
        assert_eq!(state.visible_gshells(), vec![gshells[0]]);
        assert_eq!(state.focused_gshell(), gshells[0]);
    }

    #[test]
    fn state_preserves_focus_when_closing_an_unfocused_gshell() {
        let state = WorkspaceServiceState::with_workspace(Workspace::two_pane());
        let gshells = state.visible_gshells();

        assert_eq!(
            state.close_gshell(gshells[0]),
            Some(WorkspaceGShellCloseOutcome::Closed {
                closed_gshells: vec![gshells[0]],
                focused_gshell: gshells[1],
            })
        );
        assert_eq!(state.visible_gshells(), vec![gshells[1]]);
    }

    #[test]
    fn state_requests_workspace_close_for_the_last_gshell() {
        let state = WorkspaceServiceState::new();
        let only = state.focused_gshell();

        assert_eq!(
            state.close_gshell(only),
            Some(WorkspaceGShellCloseOutcome::CloseWorkspace)
        );
        assert_eq!(state.close_gshell(GShellId::new(99)), None);
        assert_eq!(state.visible_gshells(), vec![only]);
    }

    #[test]
    fn tabs_keep_distinct_gshell_bindings_for_equal_local_pane_ids() {
        let state = WorkspaceServiceState::new();
        let first = state.focused_gshell();
        let second = state.create_tab_gshell();

        assert_ne!(first, second);
        assert_eq!(state.tab_count(), 2);
        assert_eq!(state.visible_gshells(), vec![second]);
        assert_eq!(state.activate_previous_tab(), first);
        assert_eq!(state.visible_gshells(), vec![first]);
        assert_eq!(state.activate_next_tab(), second);
    }

    #[test]
    fn tab_titles_follow_the_focused_gshell_and_fall_back_to_the_working_directory() {
        let state = WorkspaceServiceState::new();
        let first = state.focused_gshell();
        let second = state.create_tab_gshell();

        let fallback = state.default_tab_title.clone();
        assert_eq!(state.tab_titles(), vec![fallback.clone(), fallback.clone()]);

        state.update_gshell_title(first, Some("nvim".to_string()));
        state.update_gshell_title(second, Some("yazi".to_string()));
        assert_eq!(state.tab_titles(), vec!["nvim", "yazi"]);

        state.update_gshell_title(second, None);
        assert_eq!(state.tab_titles(), vec!["nvim", fallback.as_str()]);
        assert!(!state.tab_titles().iter().any(|title| title == "Shell"));
    }

    #[test]
    fn closing_a_tabs_only_gshell_closes_the_tab_and_focuses_its_neighbor() {
        let state = WorkspaceServiceState::new();
        let first = state.focused_gshell();
        let second = state.create_tab_gshell();

        assert_eq!(
            state.close_gshell(second),
            Some(WorkspaceGShellCloseOutcome::Closed {
                closed_gshells: vec![second],
                focused_gshell: first,
            })
        );
        assert_eq!(state.tab_count(), 1);
        assert_eq!(state.visible_gshells(), vec![first]);
    }

    #[test]
    fn closing_a_hidden_tab_does_not_change_the_active_tab() {
        let state = WorkspaceServiceState::new();
        let hidden = state.focused_gshell();
        let active = state.create_tab_gshell();

        assert_eq!(
            state.close_gshell(hidden),
            Some(WorkspaceGShellCloseOutcome::Closed {
                closed_gshells: vec![hidden],
                focused_gshell: active,
            })
        );
        assert_eq!(state.tab_count(), 1);
        assert_eq!(state.visible_gshells(), vec![active]);
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
