use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use germinal_ports::{
    pty_host::{size_info::TerminalSizeInfo, window_size::TerminalWindowSize},
    rendering::{
        render_target_id::RenderTargetId,
        surface_snapshot::{
            RenderSurfaceImePreeditSnapshot, RenderSurfaceSnapshot, merge_surface_dirty_rows,
        },
        surface_snapshot_mailbox::{
            SurfaceSnapshotReceiver, SurfaceSnapshotSender, surface_snapshot_mailbox,
        },
        tab_bar::TabBarSnapshot,
        window_runtime::{IRenderRuntimeStore, ITerminalWindowRuntime},
        workspace_layout::RenderSurfacePlacement,
    },
    seq::Seq,
    service::render_service::IRenderService,
};

#[derive(kudi::DepInj)]
#[target(RenderService)]
pub struct RenderServiceState {
    redraw_pending: bool,
    window_focused: bool,
    focused_render_target: Option<RenderTargetId>,
    retired_render_targets: HashSet<RenderTargetId>,
    latest_surface_seqs: RefCell<HashMap<RenderTargetId, Seq>>,
    ime_preedits: HashMap<RenderTargetId, RenderSurfaceImePreeditSnapshot>,
    surface_snapshot_tx: SurfaceSnapshotSender,
    surface_snapshot_rx: SurfaceSnapshotReceiver,
    snapshot_wake_pending: Arc<AtomicBool>,
}

impl RenderServiceState {
    pub fn new() -> Self {
        let (surface_snapshot_tx, surface_snapshot_rx) = surface_snapshot_mailbox();

        Self {
            redraw_pending: false,
            window_focused: true,
            focused_render_target: None,
            retired_render_targets: HashSet::new(),
            latest_surface_seqs: RefCell::new(HashMap::new()),
            ime_preedits: HashMap::new(),
            surface_snapshot_tx,
            surface_snapshot_rx,
            snapshot_wake_pending: Arc::new(AtomicBool::new(false)),
        }
    }

    fn take_latest_surface_snapshots(&self) -> Vec<RenderSurfaceSnapshot> {
        let mut latest_by_target = HashMap::<RenderTargetId, RenderSurfaceSnapshot>::new();

        loop {
            while let Ok(snapshot) = self.surface_snapshot_rx.try_recv() {
                self.keep_latest_surface_snapshot(&mut latest_by_target, snapshot);
            }

            // Clear only after draining. If a producer raced with the drain while the flag was
            // still true, it skipped dispatching another FrameReady. The second receive closes
            // that race: either we consume the raced snapshot and drain again, or a later
            // producer observes false and dispatches a fresh wakeup.
            self.snapshot_wake_pending.store(false, Ordering::Release);
            let Ok(snapshot) = self.surface_snapshot_rx.try_recv() else {
                break;
            };
            self.keep_latest_surface_snapshot(&mut latest_by_target, snapshot);
        }

        let mut latest_surface_seqs = self.latest_surface_seqs.borrow_mut();
        latest_by_target.retain(|target_id, snapshot| {
            if latest_surface_seqs
                .get(target_id)
                .is_some_and(|latest_seq| *latest_seq >= snapshot.latest_seq)
            {
                return false;
            }

            latest_surface_seqs.insert(*target_id, snapshot.latest_seq);
            true
        });

        latest_by_target.into_values().collect()
    }

    fn keep_latest_surface_snapshot(
        &self,
        latest_by_target: &mut HashMap<RenderTargetId, RenderSurfaceSnapshot>,
        mut snapshot: RenderSurfaceSnapshot,
    ) {
        if self.retired_render_targets.contains(&snapshot.target_id) {
            return;
        }

        if let Some(current) = latest_by_target.get(&snapshot.target_id) {
            if snapshot.latest_seq < current.latest_seq {
                return;
            }
            merge_surface_dirty_rows(&mut snapshot.dirty_rows, &current.dirty_rows);
        }

        latest_by_target.insert(snapshot.target_id, snapshot);
    }

    fn set_window_focused(&mut self, focused: bool) -> bool {
        if self.window_focused == focused {
            return false;
        }

        self.window_focused = focused;
        true
    }

    fn set_focused_render_target(&mut self, target_id: RenderTargetId) -> bool {
        if self.focused_render_target == Some(target_id) {
            return false;
        }

        self.focused_render_target = Some(target_id);
        true
    }

    fn retire_render_target(&mut self, target_id: RenderTargetId) {
        self.retired_render_targets.insert(target_id);
        if self.focused_render_target == Some(target_id) {
            self.focused_render_target = None;
        }
    }

    fn apply_cursor_focus(&self, snapshot: &mut RenderSurfaceSnapshot) {
        if let Some(cursor) = snapshot.cursor.as_mut() {
            cursor.focused =
                self.window_focused && self.focused_render_target == Some(snapshot.target_id);
        }
    }

    fn apply_ime_preedit(&self, snapshot: &mut RenderSurfaceSnapshot) {
        snapshot.ime_preedit = self.ime_preedits.get(&snapshot.target_id).cloned();
    }

    fn request_redraw(&mut self) {
        self.redraw_pending = true;
    }
}

impl Default for RenderServiceState {
    fn default() -> Self {
        Self::new()
    }
}

impl<Deps> IRenderService for RenderService<Deps>
where
    Deps: AsRef<RenderServiceState> + AsMut<RenderServiceState> + IRenderRuntimeStore,
{
    fn prepare_render_backend(&mut self) {
        let state: &mut RenderServiceState = self.prj_ref_mut().as_mut();
        state.request_redraw();
    }

    fn surface_snapshot_sender(&self) -> SurfaceSnapshotSender {
        let state: &RenderServiceState = self.prj_ref().as_ref();
        state.surface_snapshot_tx.clone()
    }

    fn snapshot_wake_pending(&self) -> Arc<AtomicBool> {
        let state: &RenderServiceState = self.prj_ref().as_ref();
        Arc::clone(&state.snapshot_wake_pending)
    }

    fn consume_latest_terminal_snapshot(&mut self) {
        let snapshots = {
            let state: &RenderServiceState = self.prj_ref().as_ref();
            state.take_latest_surface_snapshots()
        };

        if snapshots.is_empty() {
            return;
        }

        for mut snapshot in snapshots {
            {
                let state: &RenderServiceState = self.prj_ref().as_ref();
                state.apply_cursor_focus(&mut snapshot);
                state.apply_ime_preedit(&mut snapshot);
            }

            self.prj_ref_mut()
                .window_runtime_mut()
                .expect("window runtime must be initialized before use")
                .set_surface_snapshot(snapshot);
        }
    }

    fn current_terminal_size_info(&self) -> TerminalSizeInfo {
        self.prj_ref()
            .window_runtime()
            .expect("window runtime must be initialized before use")
            .terminal_size_info()
    }

    fn terminal_size_info_for_surface(
        &self,
        placement: RenderSurfacePlacement,
    ) -> TerminalSizeInfo {
        self.prj_ref()
            .window_runtime()
            .expect("window runtime must be initialized before use")
            .terminal_size_info_for_window_size(placement.window_size())
    }

    fn set_workspace_render_layout(&mut self, placements: Vec<RenderSurfacePlacement>) {
        self.prj_ref_mut()
            .window_runtime_mut()
            .expect("window runtime must be initialized before use")
            .set_workspace_layout(placements);
        self.prj_ref_mut().as_mut().redraw_pending = true;
    }

    fn set_tab_bar(&mut self, tab_bar: Option<TabBarSnapshot>) {
        self.prj_ref_mut()
            .window_runtime_mut()
            .expect("window runtime must be initialized before use")
            .set_tab_bar(tab_bar);
        self.prj_ref_mut().as_mut().redraw_pending = true;
    }

    fn tab_index_at_position(&self, x_px: f64, y_px: f64) -> Option<usize> {
        self.prj_ref()
            .window_runtime()
            .expect("window runtime must be initialized before use")
            .tab_index_at_position(x_px, y_px)
    }

    fn set_window_title(&mut self, title: &str) {
        self.prj_ref_mut()
            .window_runtime_mut()
            .expect("window runtime must be initialized before use")
            .set_window_title(title);
    }

    fn ring_bell(&mut self, visual_duration: Duration, request_attention: bool) {
        self.prj_ref_mut()
            .window_runtime_mut()
            .expect("window runtime must be initialized before use")
            .ring_bell(visual_duration, request_attention);
        self.prj_ref_mut().as_mut().redraw_pending = true;
    }

    fn resize_window_size_info(&mut self, window_size: TerminalWindowSize) -> TerminalSizeInfo {
        let size_info = self
            .prj_ref_mut()
            .window_runtime_mut()
            .expect("window runtime must be initialized before use")
            .resize_surface_size_info(window_size);

        self.prj_ref_mut().as_mut().redraw_pending = true;

        size_info
    }

    fn set_window_focused(&mut self, focused: bool) {
        let changed = {
            let state: &mut RenderServiceState = self.prj_ref_mut().as_mut();
            state.set_window_focused(focused)
        };

        if !changed {
            return;
        }

        refresh_cursor_focus(self.prj_ref_mut());
    }

    fn set_focused_render_target(&mut self, target_id: RenderTargetId) {
        let changed = {
            let state: &mut RenderServiceState = self.prj_ref_mut().as_mut();
            state.set_focused_render_target(target_id)
        };

        if !changed {
            return;
        }

        refresh_cursor_focus(self.prj_ref_mut());
    }

    fn set_ime_preedit(
        &mut self,
        target_id: RenderTargetId,
        preedit: Option<RenderSurfaceImePreeditSnapshot>,
    ) {
        {
            let state: &mut RenderServiceState = self.prj_ref_mut().as_mut();
            if let Some(preedit) = preedit.clone() {
                state.ime_preedits.insert(target_id, preedit);
            } else {
                state.ime_preedits.remove(&target_id);
            }
            state.redraw_pending = true;
        }

        let runtime = self
            .prj_ref_mut()
            .window_runtime_mut()
            .expect("window runtime must be initialized before use");
        for snapshot in runtime.surface_snapshots_mut() {
            if snapshot.target_id == target_id {
                snapshot.ime_preedit = preedit.clone();
                break;
            }
        }
    }

    fn reset_surface_sequence(&mut self, target_id: RenderTargetId) {
        let state: &mut RenderServiceState = self.prj_ref_mut().as_mut();
        state.latest_surface_seqs.borrow_mut().remove(&target_id);
    }

    fn remove_render_target(&mut self, target_id: RenderTargetId) {
        {
            let state: &mut RenderServiceState = self.prj_ref_mut().as_mut();
            state.retire_render_target(target_id);
            state.ime_preedits.remove(&target_id);
        }

        self.prj_ref_mut()
            .window_runtime_mut()
            .expect("window runtime must be initialized before use")
            .remove_render_target(target_id);
        self.prj_ref_mut().as_mut().redraw_pending = true;
    }

    fn request_redraw(&mut self) {
        let state: &mut RenderServiceState = self.prj_ref_mut().as_mut();
        state.request_redraw();
    }

    fn flush_redraw_request(&mut self) {
        let should_request = {
            let state: &mut RenderServiceState = self.prj_ref_mut().as_mut();
            let should_request = state.redraw_pending;
            state.redraw_pending = false;
            should_request
        };

        let runtime = self
            .prj_ref_mut()
            .window_runtime_mut()
            .expect("window runtime must be initialized before use");

        if should_request {
            runtime.schedule_redraw();
        }
        if runtime.take_redraw_request() {
            runtime.request_window_redraw();
        }
    }

    fn present_workspace(&mut self) {
        self.prj_ref_mut()
            .window_runtime_mut()
            .expect("window runtime must be initialized before use")
            .render();
    }
}

fn refresh_cursor_focus<Deps>(deps: &mut Deps)
where
    Deps: AsRef<RenderServiceState> + AsMut<RenderServiceState> + IRenderRuntimeStore,
{
    let (window_focused, focused_render_target) = {
        let state: &RenderServiceState = deps.as_ref();
        (state.window_focused, state.focused_render_target)
    };

    let runtime = deps
        .window_runtime_mut()
        .expect("window runtime must be initialized before use");

    for snapshot in runtime.surface_snapshots_mut() {
        let target_id = snapshot.target_id;
        if let Some(cursor) = snapshot.cursor.as_mut() {
            cursor.focused = window_focused && focused_render_target == Some(target_id);
        }
    }

    deps.as_mut().redraw_pending = true;
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use germinal_ports::{
        rendering::{
            render_target_id::RenderTargetId,
            surface_snapshot::{RenderSurfaceImePreeditSnapshot, RenderSurfaceSnapshot},
        },
        seq::Seq,
    };

    use super::RenderServiceState;

    fn snapshot(target: u64, seq: u64) -> RenderSurfaceSnapshot {
        RenderSurfaceSnapshot {
            target_id: RenderTargetId::new(target),
            latest_seq: Seq::new(seq),
            default_background: germinal_ports::rendering::frame_plan_builder::RgbColorDto::new(
                0, 0, 0,
            ),
            rows: Vec::new(),
            image_surfaces: Vec::new(),
            dirty_rows: Vec::new(),
            cursor: None,
            ime_preedit: None,
        }
    }

    #[test]
    fn draining_snapshots_keeps_latest_update_for_every_target() {
        let state = RenderServiceState::new();
        state
            .surface_snapshot_tx
            .send(snapshot(1, 1))
            .expect("first target snapshot");
        state
            .surface_snapshot_tx
            .send(snapshot(2, 4))
            .expect("second target snapshot");
        state
            .surface_snapshot_tx
            .send(snapshot(1, 3))
            .expect("newer first target snapshot");

        let mut snapshots = state.take_latest_surface_snapshots();
        snapshots.sort_by_key(|snapshot| snapshot.target_id.value());

        assert_eq!(snapshots.len(), 2);
        assert_eq!(snapshots[0].target_id, RenderTargetId::new(1));
        assert_eq!(snapshots[0].latest_seq, Seq::new(3));
        assert_eq!(snapshots[1].target_id, RenderTargetId::new(2));
        assert_eq!(snapshots[1].latest_seq, Seq::new(4));
    }

    #[test]
    fn draining_snapshots_merges_damage_from_coalesced_updates() {
        let state = RenderServiceState::new();
        let mut first = snapshot(1, 1);
        first.dirty_rows = vec![1, 2];
        let mut second = snapshot(1, 2);
        second.dirty_rows = vec![4, 5];
        state
            .surface_snapshot_tx
            .send(first)
            .expect("first damaged snapshot");
        state
            .surface_snapshot_tx
            .send(second)
            .expect("second damaged snapshot");

        let snapshots = state.take_latest_surface_snapshots();

        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].latest_seq, Seq::new(2));
        assert_eq!(snapshots[0].dirty_rows, vec![1, 2, 4, 5]);
    }

    #[test]
    fn draining_snapshots_rearms_the_worker_wakeup() {
        let state = RenderServiceState::new();
        state.snapshot_wake_pending.store(true, Ordering::Release);
        state
            .surface_snapshot_tx
            .send(snapshot(1, 1))
            .expect("queued snapshot");

        assert_eq!(state.take_latest_surface_snapshots().len(), 1);
        assert!(!state.snapshot_wake_pending.load(Ordering::Acquire));
    }

    #[test]
    fn draining_snapshots_does_not_replace_newer_seq_with_stale_update() {
        let state = RenderServiceState::new();
        state
            .surface_snapshot_tx
            .send(snapshot(7, 5))
            .expect("new snapshot");
        state
            .surface_snapshot_tx
            .send(snapshot(7, 2))
            .expect("stale snapshot");

        let snapshots = state.take_latest_surface_snapshots();

        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].latest_seq, Seq::new(5));
    }

    #[test]
    fn draining_snapshots_rejects_a_stale_update_from_a_later_drain() {
        let state = RenderServiceState::new();
        state
            .surface_snapshot_tx
            .send(snapshot(7, 5))
            .expect("new snapshot");
        assert_eq!(state.take_latest_surface_snapshots().len(), 1);

        state
            .surface_snapshot_tx
            .send(snapshot(7, 2))
            .expect("late stale snapshot");

        assert!(state.take_latest_surface_snapshots().is_empty());
    }

    #[test]
    fn draining_snapshots_discards_updates_for_retired_targets() {
        let mut state = RenderServiceState::new();
        state
            .surface_snapshot_tx
            .send(snapshot(1, 1))
            .expect("retired target snapshot");
        state
            .surface_snapshot_tx
            .send(snapshot(2, 1))
            .expect("live target snapshot");
        state.retire_render_target(RenderTargetId::new(1));

        let snapshots = state.take_latest_surface_snapshots();

        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].target_id, RenderTargetId::new(2));
    }

    #[test]
    fn ime_preedit_state_is_applied_only_to_its_render_target() {
        let target_id = RenderTargetId::new(1);
        let preedit = RenderSurfaceImePreeditSnapshot {
            text: "拼音".to_string(),
            cursor_range: Some((3, 3)),
        };
        let mut state = RenderServiceState::new();
        state.ime_preedits.insert(target_id, preedit.clone());
        let mut first = snapshot(1, 1);
        let mut second = snapshot(2, 1);

        state.apply_ime_preedit(&mut first);
        state.apply_ime_preedit(&mut second);

        assert_eq!(first.ime_preedit, Some(preedit));
        assert_eq!(second.ime_preedit, None);
    }
}
