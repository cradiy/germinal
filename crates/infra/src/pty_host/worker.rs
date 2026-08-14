use std::{
    env,
    num::NonZeroUsize,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, Receiver, Sender, SyncSender, TryRecvError},
    },
    thread,
    time::{Duration, Instant},
};

use germinal_domain::{gshell::vo::gshell_id::GShellId, pty_host::terminal_size::TerminalGridSize};
use germinal_ports::{
    event::{
        runtime_event::{GShellRuntimeEvent, RuntimeEvent},
        runtime_event_dispatcher::IRuntimeEventDispatcher,
    },
    pty_host::{
        pty_input::{PtyInput, PtyInputSender},
        snapshot::TerminalSnapshotProvider,
        terminal_input_mode::TerminalInputModeState,
        worker_backend::ITerminalWorkerBackend,
        worker_input::{
            TerminalDisplayScroll, TerminalSelectionKind, TerminalSelectionPoint, TerminalViMotion,
            TerminalViSearchDirection, TerminalViSearchPrompt, TerminalViSelectionKind,
            TerminalViTextObject, TerminalWorkerInput,
        },
    },
    rendering::{render_target_id::RenderTargetId, surface_snapshot::RenderSurfaceSnapshot},
    seq::Seq,
};
use rayon::ThreadPool;
use tracing::{debug, error, info};

use crate::{
    gnative::control_sequence::GNativeEnterControlSequenceDecoder,
    pty_host::alacritty_terminal_store::{AlacrittyTermSize, AlacrittyTerminalStore},
};

const TERMINAL_INPUT_CHANNEL_CAPACITY: usize = 64;
const MAX_PENDING_BYTES_BEFORE_APPLY: usize = 256 * 1024;
const MAX_EVENTS_PER_WORKER_TICK: usize = 256;
const PUBLISH_RETRY_INTERVAL: Duration = Duration::from_millis(2);
const PERF_LOG_INTERVAL: Duration = Duration::from_secs(1);
const TERMINAL_WORKER_PERF_LOG_ENV: &str = "GERMINAL_TERMINAL_WORKER_PERF_LOG";
const TERMINAL_WORKER_POOL_ENV: &str = "GERMINAL_TERMINAL_WORKER_THREADS";

struct TerminalWorkerRegistration<Dispatch> {
    proxy: Dispatch,
    gshell_id: GShellId,
    initial_size: TerminalGridSize,
    scrollback_history: usize,
    surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
    snapshot_wake_pending: Arc<AtomicBool>,
    input_rx: Receiver<TerminalWorkerInput>,
}

struct TerminalWorkerHandle<Dispatch> {
    runtime: TerminalWorkerRuntime<Dispatch>,
    input_rx: Receiver<TerminalWorkerInput>,
}

enum TerminalWorkerTick {
    Idle,
    Progressed,
    Finished,
}

impl<Dispatch> TerminalWorkerHandle<Dispatch>
where
    Dispatch: IRuntimeEventDispatcher,
{
    fn new(registration: TerminalWorkerRegistration<Dispatch>) -> Self {
        Self {
            runtime: TerminalWorkerRuntime::new(
                registration.proxy,
                registration.gshell_id,
                to_alacritty_term_size(registration.initial_size),
                registration.scrollback_history,
                registration.surface_snapshot_tx,
                registration.snapshot_wake_pending,
            ),
            input_rx: registration.input_rx,
        }
    }

    fn tick(&mut self) -> TerminalWorkerTick {
        let mut progressed = false;
        let mut disconnected = false;

        match self.input_rx.try_recv() {
            Ok(first_input) => {
                self.runtime.process_batch(first_input, &self.input_rx);
                if self.runtime.pending_bytes_len >= MAX_PENDING_BYTES_BEFORE_APPLY {
                    self.runtime.flush_pending_input();
                }
                self.runtime.flush_pending_input();
                progressed = true;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => disconnected = true,
        }

        progressed |= self.runtime.publish_unpublished_snapshot();

        if disconnected {
            self.runtime.flush_pending_input();
            let published = self.runtime.publish_unpublished_snapshot();

            if self.runtime.unpublished_seq.is_none() {
                self.runtime.perf.maybe_force_log();
                return TerminalWorkerTick::Finished;
            }

            progressed |= published;
        }

        self.runtime.perf.maybe_log();

        if progressed {
            TerminalWorkerTick::Progressed
        } else {
            TerminalWorkerTick::Idle
        }
    }
}

struct TerminalWorkerPool<Dispatch> {
    _thread_pool: Option<ThreadPool>,
    registration_txs: Vec<Sender<TerminalWorkerRegistration<Dispatch>>>,
    next_lane: AtomicUsize,
}

impl<Dispatch> TerminalWorkerPool<Dispatch>
where
    Dispatch: IRuntimeEventDispatcher,
{
    fn new(worker_count: usize) -> Self {
        let worker_count = worker_count.max(1);
        let thread_pool = rayon::ThreadPoolBuilder::new()
            .num_threads(worker_count)
            .thread_name(|lane_index| format!("terminal-worker-{lane_index}"))
            .build();
        if let Err(error) = &thread_pool {
            error!(error = %error, "failed to build terminal worker thread pool; falling back to std threads");
        }
        let thread_pool = thread_pool.ok();
        let mut registration_txs = Vec::with_capacity(worker_count);

        for lane_index in 0..worker_count {
            let (registration_tx, registration_rx) =
                mpsc::channel::<TerminalWorkerRegistration<Dispatch>>();

            if let Some(thread_pool) = thread_pool.as_ref() {
                thread_pool.spawn_fifo(move || run_terminal_worker_lane(registration_rx));
            } else if let Err(error) = thread::Builder::new()
                .name(format!("terminal-worker-fallback-{lane_index}"))
                .spawn(move || run_terminal_worker_lane(registration_rx))
            {
                error!(error = %error, lane_index, "failed to spawn fallback terminal worker lane");
            }

            registration_txs.push(registration_tx);
        }

        Self {
            _thread_pool: thread_pool,
            registration_txs,
            next_lane: AtomicUsize::new(0),
        }
    }

    fn spawn_terminal_worker(
        &self,
        proxy: Dispatch,
        gshell_id: GShellId,
        initial_size: TerminalGridSize,
        scrollback_history: usize,
        surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
        snapshot_wake_pending: Arc<AtomicBool>,
    ) -> SyncSender<TerminalWorkerInput> {
        let (tx, rx) = mpsc::sync_channel::<TerminalWorkerInput>(TERMINAL_INPUT_CHANNEL_CAPACITY);
        let lane_index =
            self.next_lane.fetch_add(1, Ordering::Relaxed) % self.registration_txs.len();
        let registration = TerminalWorkerRegistration {
            proxy,
            gshell_id,
            initial_size,
            scrollback_history,
            surface_snapshot_tx,
            snapshot_wake_pending,
            input_rx: rx,
        };

        if let Err(error) = self.registration_txs[lane_index].send(registration) {
            error!(error = %error, lane_index, "failed to register terminal worker with worker lane");
        }

        tx
    }
}

fn run_terminal_worker_lane<Dispatch>(
    registration_rx: Receiver<TerminalWorkerRegistration<Dispatch>>,
) where
    Dispatch: IRuntimeEventDispatcher,
{
    let mut workers = Vec::<TerminalWorkerHandle<Dispatch>>::new();
    let mut registrations_open = true;

    loop {
        let mut progressed = false;

        if workers.is_empty() && registrations_open {
            match registration_rx.recv() {
                Ok(registration) => {
                    workers.push(TerminalWorkerHandle::new(registration));
                    progressed = true;
                }
                Err(_) => break,
            }
        }

        loop {
            match registration_rx.try_recv() {
                Ok(registration) => {
                    workers.push(TerminalWorkerHandle::new(registration));
                    progressed = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    registrations_open = false;
                    break;
                }
            }
        }

        let mut index = 0;
        while index < workers.len() {
            match workers[index].tick() {
                TerminalWorkerTick::Idle => index += 1,
                TerminalWorkerTick::Progressed => {
                    progressed = true;
                    index += 1;
                }
                TerminalWorkerTick::Finished => {
                    progressed = true;
                    workers.swap_remove(index);
                }
            }
        }

        if !registrations_open && workers.is_empty() {
            break;
        }

        if !progressed {
            thread::park_timeout(PUBLISH_RETRY_INTERVAL);
        }
    }
}

struct TerminalWorkerRuntime<Dispatch> {
    proxy: Dispatch,

    gshell_id: GShellId,
    target_id: RenderTargetId,
    seq: u64,

    terminal_store: AlacrittyTerminalStore,

    surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
    snapshot_wake_pending: Arc<AtomicBool>,

    pending_chunks: Vec<Vec<u8>>,
    pending_bytes_len: usize,

    unpublished_seq: Option<Seq>,

    perf: TerminalWorkerPerf,

    pty_input_tx: Option<PtyInputSender>,
    input_modes: Option<TerminalInputModeState>,
    gnative_enter_decoder: GNativeEnterControlSequenceDecoder,
}

impl<Dispatch> TerminalWorkerRuntime<Dispatch>
where
    Dispatch: IRuntimeEventDispatcher,
{
    fn new(
        proxy: Dispatch,
        gshell_id: GShellId,
        initial_size: AlacrittyTermSize,
        scrollback_history: usize,
        surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
        snapshot_wake_pending: Arc<AtomicBool>,
    ) -> Self {
        let target_id = RenderTargetId::new(gshell_id.value());
        let terminal_store = AlacrittyTerminalStore::with_size_and_scrollback_history(
            initial_size,
            scrollback_history,
        );

        Self {
            proxy,

            gshell_id,
            target_id,
            seq: 0,

            terminal_store,

            surface_snapshot_tx,
            snapshot_wake_pending,

            pending_chunks: Vec::new(),
            pending_bytes_len: 0,

            unpublished_seq: None,

            perf: TerminalWorkerPerf::new(),

            pty_input_tx: None,
            input_modes: None,
            gnative_enter_decoder: GNativeEnterControlSequenceDecoder::default(),
        }
    }

    fn process_batch(
        &mut self,
        first_input: TerminalWorkerInput,
        rx: &Receiver<TerminalWorkerInput>,
    ) {
        self.collect_input(first_input);

        for _ in 0..MAX_EVENTS_PER_WORKER_TICK {
            if self.pending_bytes_len >= MAX_PENDING_BYTES_BEFORE_APPLY {
                break;
            }

            match rx.try_recv() {
                Ok(input) => {
                    self.collect_input(input);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
    }

    fn collect_input(&mut self, input: TerminalWorkerInput) {
        match input {
            TerminalWorkerInput::Bytes(bytes) => {
                self.perf.input_chunks += 1;
                self.perf.input_bytes += bytes.len() as u64;

                self.pending_bytes_len += bytes.len();
                self.pending_chunks.push(bytes);
            }
            TerminalWorkerInput::Resize(size) => {
                self.flush_pending_input();
                self.unpublished_seq = Some(self.resize(to_alacritty_term_size(size)));
            }
            TerminalWorkerInput::ScrollDisplay(scroll) => {
                self.flush_pending_input();
                if let Some(seq) = self.scroll_display(scroll) {
                    self.unpublished_seq = Some(seq);
                }
            }
            TerminalWorkerInput::StartSelection { kind, point } => {
                self.flush_pending_input();
                if let Some(seq) = self.start_selection(kind, point) {
                    self.unpublished_seq = Some(seq);
                }
            }
            TerminalWorkerInput::UpdateSelection(point) => {
                self.flush_pending_input();
                if let Some(seq) = self.update_selection(point) {
                    self.unpublished_seq = Some(seq);
                }
            }
            TerminalWorkerInput::RequestSelectionText => {
                self.flush_pending_input();
                self.dispatch_selection_text();
            }
            TerminalWorkerInput::SetViMode(enabled) => {
                self.flush_pending_input();
                if let Some(seq) = self.set_vi_mode(enabled) {
                    self.unpublished_seq = Some(seq);
                }
            }
            TerminalWorkerInput::ViMotion(motion) => {
                self.flush_pending_input();
                if let Some(seq) = self.vi_motion(motion) {
                    self.unpublished_seq = Some(seq);
                }
            }
            TerminalWorkerInput::SetViSelection(kind) => {
                self.flush_pending_input();
                if let Some(seq) = self.set_vi_selection(kind) {
                    self.unpublished_seq = Some(seq);
                }
            }
            TerminalWorkerInput::SelectViTextObject(text_object) => {
                self.flush_pending_input();
                if let Some(seq) = self.select_vi_text_object(text_object) {
                    self.unpublished_seq = Some(seq);
                }
            }
            TerminalWorkerInput::SetViSearchPrompt(prompt) => {
                self.flush_pending_input();
                if let Some(seq) = self.set_vi_search_prompt(prompt) {
                    self.unpublished_seq = Some(seq);
                }
            }
            TerminalWorkerInput::ViSearch { pattern, direction } => {
                self.flush_pending_input();
                if let Some(seq) = self.vi_search(&pattern, direction) {
                    self.unpublished_seq = Some(seq);
                }
            }
            TerminalWorkerInput::SetPtyInput {
                sender,
                input_modes,
            } => {
                self.pty_input_tx = Some(sender);
                self.input_modes = Some(input_modes);
                self.publish_input_modes();
            }
        }
    }

    fn flush_pending_input(&mut self) {
        if self.pending_chunks.is_empty() {
            return;
        }

        let chunks = std::mem::take(&mut self.pending_chunks);
        self.pending_bytes_len = 0;

        self.unpublished_seq = self.apply_byte_chunks(&chunks);
    }

    fn forward_pty_writes(&self, writes: Vec<Vec<u8>>) {
        if writes.is_empty() {
            return;
        }

        let Some(tx) = self.pty_input_tx.as_ref() else {
            return;
        };

        for bytes in writes {
            let _ = tx.send(PtyInput::Bytes(bytes));
        }
    }

    fn publish_input_modes(&self) {
        if let Some(input_modes) = &self.input_modes {
            input_modes.store(self.terminal_store.input_modes(self.target_id));
        }
    }

    fn apply_byte_chunks(&mut self, chunks: &[Vec<u8>]) -> Option<Seq> {
        self.seq += 1;

        let seq = Seq::new(self.seq);
        let started_at = Instant::now();
        let mut applied_visible_bytes = false;
        let mut enter_gnative = false;

        for bytes in chunks {
            let decode_result = self.gnative_enter_decoder.decode(bytes);
            enter_gnative |= decode_result.enter_gnative;

            if decode_result.visible_bytes.is_empty() {
                continue;
            }

            applied_visible_bytes = true;
            self.terminal_store
                .apply_bytes(self.target_id, seq, &decode_result.visible_bytes);

            let pending_pty_writes = self.terminal_store.take_pending_pty_writes(self.target_id);
            self.forward_pty_writes(pending_pty_writes);
        }
        self.publish_input_modes();
        if let Some(title) = self.terminal_store.take_title_change(self.target_id) {
            let _ = self
                .proxy
                .dispatch(RuntimeEvent::GShell(GShellRuntimeEvent::TitleChanged {
                    gshell_id: self.gshell_id,
                    title,
                }));
        }

        if enter_gnative {
            let _ = self
                .proxy
                .dispatch(RuntimeEvent::GShell(GShellRuntimeEvent::EnterGNative {
                    gshell_id: self.gshell_id,
                }));
        }

        let elapsed = started_at.elapsed();

        self.perf.apply_batches += 1;
        self.perf.apply_chunks += chunks.len() as u64;
        self.perf.apply_time += elapsed;
        self.perf.apply_max = self.perf.apply_max.max(elapsed);

        applied_visible_bytes.then_some(seq)
    }

    fn resize(&mut self, size: AlacrittyTermSize) -> Seq {
        self.seq += 1;

        let seq = Seq::new(self.seq);
        let started_at = Instant::now();

        self.terminal_store.resize(self.target_id, seq, size);

        let elapsed = started_at.elapsed();

        self.perf.resize_count += 1;
        self.perf.resize_time += elapsed;
        self.perf.resize_max = self.perf.resize_max.max(elapsed);

        seq
    }

    fn scroll_display(&mut self, scroll: TerminalDisplayScroll) -> Option<Seq> {
        self.seq += 1;

        let seq = Seq::new(self.seq);
        let scroll = match scroll {
            TerminalDisplayScroll::Delta(lines) => alacritty_terminal::grid::Scroll::Delta(lines),
            TerminalDisplayScroll::PageUp => alacritty_terminal::grid::Scroll::PageUp,
            TerminalDisplayScroll::PageDown => alacritty_terminal::grid::Scroll::PageDown,
            TerminalDisplayScroll::Top => alacritty_terminal::grid::Scroll::Top,
            TerminalDisplayScroll::Bottom => alacritty_terminal::grid::Scroll::Bottom,
        };

        self.terminal_store
            .scroll_display(self.target_id, seq, scroll)
            .then_some(seq)
    }

    fn start_selection(
        &mut self,
        kind: TerminalSelectionKind,
        point: TerminalSelectionPoint,
    ) -> Option<Seq> {
        self.seq += 1;
        let seq = Seq::new(self.seq);
        self.terminal_store
            .start_selection(self.target_id, seq, kind, point)
            .then_some(seq)
    }

    fn update_selection(&mut self, point: TerminalSelectionPoint) -> Option<Seq> {
        self.seq += 1;
        let seq = Seq::new(self.seq);
        self.terminal_store
            .update_selection(self.target_id, seq, point)
            .then_some(seq)
    }

    fn dispatch_selection_text(&self) {
        let text = self.terminal_store.selection_text(self.target_id);
        let _ = self
            .proxy
            .dispatch(RuntimeEvent::GShell(GShellRuntimeEvent::SelectionText {
                gshell_id: self.gshell_id,
                text,
            }));
    }

    fn set_vi_mode(&mut self, enabled: bool) -> Option<Seq> {
        self.seq += 1;
        let seq = Seq::new(self.seq);
        self.terminal_store
            .set_vi_mode(self.target_id, seq, enabled)
            .then_some(seq)
    }

    fn vi_motion(&mut self, motion: TerminalViMotion) -> Option<Seq> {
        self.seq += 1;
        let seq = Seq::new(self.seq);
        self.terminal_store
            .vi_motion(self.target_id, seq, motion)
            .then_some(seq)
    }

    fn set_vi_selection(&mut self, kind: Option<TerminalViSelectionKind>) -> Option<Seq> {
        self.seq += 1;
        let seq = Seq::new(self.seq);
        self.terminal_store
            .set_vi_selection(self.target_id, seq, kind)
            .then_some(seq)
    }

    fn select_vi_text_object(&mut self, text_object: TerminalViTextObject) -> Option<Seq> {
        self.seq += 1;
        let seq = Seq::new(self.seq);
        self.terminal_store
            .select_vi_text_object(self.target_id, seq, text_object)
            .then_some(seq)
    }

    fn set_vi_search_prompt(&mut self, prompt: Option<TerminalViSearchPrompt>) -> Option<Seq> {
        self.seq += 1;
        let seq = Seq::new(self.seq);
        self.terminal_store
            .set_vi_search_prompt(self.target_id, seq, prompt)
            .then_some(seq)
    }

    fn vi_search(&mut self, pattern: &str, direction: TerminalViSearchDirection) -> Option<Seq> {
        self.seq += 1;
        let seq = Seq::new(self.seq);
        self.terminal_store
            .vi_search(self.target_id, seq, pattern, direction)
            .then_some(seq)
    }

    fn publish_unpublished_snapshot(&mut self) -> bool {
        if self.unpublished_seq.is_none() {
            return false;
        }

        if self
            .terminal_store
            .finish_expired_synchronized_update(self.target_id, Instant::now())
        {
            self.publish_input_modes();
        }
        if self
            .terminal_store
            .synchronized_update_pending(self.target_id)
        {
            return false;
        }

        if self.snapshot_wake_pending.load(Ordering::Acquire) {
            self.perf.coalesced_wakeups += 1;
            return false;
        }

        let Some(seq) = self.unpublished_seq.take() else {
            return false;
        };

        let started_at = Instant::now();
        let snapshot_started_at = Instant::now();
        let Some(mut snapshot) = self
            .terminal_store
            .render_surface_snapshot_of(self.target_id)
        else {
            debug!(
                gshell_id = self.gshell_id.value(),
                seq = seq.value(),
                "no terminal surface snapshot to publish"
            );
            self.unpublished_seq = Some(seq);
            return false;
        };
        self.perf.publish_snapshot += snapshot_started_at.elapsed();

        let cursor_started_at = Instant::now();
        snapshot.cursor = self.terminal_store.cursor_snapshot(self.target_id);
        self.perf.publish_cursor += cursor_started_at.elapsed();

        let send_started_at = Instant::now();
        let snapshot_sent = self.surface_snapshot_tx.send(snapshot).is_ok();
        let wake_already_pending =
            snapshot_sent && self.snapshot_wake_pending.swap(true, Ordering::AcqRel);
        self.perf.publish_send += send_started_at.elapsed();

        let clear_started_at = Instant::now();
        self.terminal_store.clear_damage_up_to(self.target_id, seq);
        self.perf.publish_clear += clear_started_at.elapsed();

        let elapsed = started_at.elapsed();

        self.perf.publish_count += 1;
        self.perf.publish_time += elapsed;
        self.perf.publish_max = self.perf.publish_max.max(elapsed);

        if !snapshot_sent {
            return true;
        }

        if wake_already_pending {
            self.perf.coalesced_wakeups += 1;
            self.unpublished_seq = Some(seq);
            return true;
        }

        let dispatch_started_at = Instant::now();
        let _ = self
            .proxy
            .dispatch(RuntimeEvent::GShell(GShellRuntimeEvent::FrameReady {
                gshell_id: self.gshell_id,
                seq,
            }));
        self.perf.publish_dispatch += dispatch_started_at.elapsed();
        true
    }
}

struct TerminalWorkerPerf {
    logging_enabled: bool,
    started_at: Instant,
    last_log_at: Instant,

    input_chunks: u64,
    input_bytes: u64,

    apply_batches: u64,
    apply_chunks: u64,
    apply_time: Duration,
    apply_max: Duration,

    publish_count: u64,
    publish_time: Duration,
    publish_max: Duration,
    publish_snapshot: Duration,
    publish_cursor: Duration,
    publish_send: Duration,
    publish_clear: Duration,
    publish_dispatch: Duration,

    resize_count: u64,
    resize_time: Duration,
    resize_max: Duration,

    coalesced_wakeups: u64,
}

impl TerminalWorkerPerf {
    fn new() -> Self {
        let now = Instant::now();

        Self {
            logging_enabled: terminal_worker_perf_logging_enabled(),
            started_at: now,
            last_log_at: now,

            input_chunks: 0,
            input_bytes: 0,

            apply_batches: 0,
            apply_chunks: 0,
            apply_time: Duration::ZERO,
            apply_max: Duration::ZERO,

            publish_count: 0,
            publish_time: Duration::ZERO,
            publish_max: Duration::ZERO,
            publish_snapshot: Duration::ZERO,
            publish_cursor: Duration::ZERO,
            publish_send: Duration::ZERO,
            publish_clear: Duration::ZERO,
            publish_dispatch: Duration::ZERO,

            resize_count: 0,
            resize_time: Duration::ZERO,
            resize_max: Duration::ZERO,

            coalesced_wakeups: 0,
        }
    }

    fn maybe_log(&mut self) {
        if !self.logging_enabled {
            return;
        }

        if self.last_log_at.elapsed() < PERF_LOG_INTERVAL {
            return;
        }

        self.log_and_reset();
    }

    fn maybe_force_log(&mut self) {
        if !self.logging_enabled {
            return;
        }

        if self.input_chunks == 0
            && self.apply_batches == 0
            && self.publish_count == 0
            && self.resize_count == 0
        {
            return;
        }

        self.log_and_reset();
    }

    fn log_and_reset(&mut self) {
        let elapsed = self.last_log_at.elapsed();
        let elapsed_secs = elapsed.as_secs_f64().max(0.001);

        let mib = self.input_bytes as f64 / 1024.0 / 1024.0;
        let mib_per_sec = mib / elapsed_secs;

        info!(
            "[terminal-worker] input={} chunks / {:.2} MiB / {:.2} MiB/s, apply={} batches {} chunks \
			 avg={} max={}, publish={} avg={} max={} \
			 parts(snapshot/cursor/send/clear/dispatch)={}/{}/{}/{}/{}, resize={} avg={} max={}, \
			 coalesced_wakeups={}, uptime={}",
            self.input_chunks,
            mib,
            mib_per_sec,
            self.apply_batches,
            self.apply_chunks,
            fmt_avg(self.apply_time, self.apply_batches),
            fmt_duration(self.apply_max),
            self.publish_count,
            fmt_avg(self.publish_time, self.publish_count),
            fmt_duration(self.publish_max),
            fmt_avg(self.publish_snapshot, self.publish_count),
            fmt_avg(self.publish_cursor, self.publish_count),
            fmt_avg(self.publish_send, self.publish_count),
            fmt_avg(self.publish_clear, self.publish_count),
            fmt_avg(self.publish_dispatch, self.publish_count),
            self.resize_count,
            fmt_avg(self.resize_time, self.resize_count),
            fmt_duration(self.resize_max),
            self.coalesced_wakeups,
            fmt_duration(self.started_at.elapsed()),
        );

        self.last_log_at = Instant::now();

        self.input_chunks = 0;
        self.input_bytes = 0;

        self.apply_batches = 0;
        self.apply_chunks = 0;
        self.apply_time = Duration::ZERO;
        self.apply_max = Duration::ZERO;

        self.publish_count = 0;
        self.publish_time = Duration::ZERO;
        self.publish_max = Duration::ZERO;
        self.publish_snapshot = Duration::ZERO;
        self.publish_cursor = Duration::ZERO;
        self.publish_send = Duration::ZERO;
        self.publish_clear = Duration::ZERO;
        self.publish_dispatch = Duration::ZERO;

        self.resize_count = 0;
        self.resize_time = Duration::ZERO;
        self.resize_max = Duration::ZERO;

        self.coalesced_wakeups = 0;
    }
}

fn fmt_avg(total: Duration, count: u64) -> String {
    if count == 0 {
        return "-".to_string();
    }

    fmt_duration(total / count as u32)
}

fn fmt_duration(duration: Duration) -> String {
    let micros = duration.as_micros();

    if micros < 1_000 {
        return format!("{micros}us");
    }

    let millis = duration.as_secs_f64() * 1_000.0;

    if millis < 1_000.0 {
        return format!("{millis:.2}ms");
    }

    format!("{:.2}s", duration.as_secs_f64())
}

fn terminal_worker_perf_logging_enabled() -> bool {
    env::var_os(TERMINAL_WORKER_PERF_LOG_ENV)
        .and_then(|value| value.into_string().ok())
        .is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
}

fn terminal_worker_pool_size() -> usize {
    let env_size = env::var_os(TERMINAL_WORKER_POOL_ENV)
        .and_then(|value| value.into_string().ok())
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|&value| value > 0);

    env_size
        .or_else(|| thread::available_parallelism().map(NonZeroUsize::get).ok())
        .unwrap_or(1)
}

fn to_alacritty_term_size(size: TerminalGridSize) -> AlacrittyTermSize {
    AlacrittyTermSize::new(size.columns(), size.rows())
}

pub struct PlatformTerminalWorkerBackend<Dispatch> {
    proxy: Dispatch,
    scrollback_history: usize,
    pool: OnceLock<TerminalWorkerPool<Dispatch>>,
}

impl<Dispatch> PlatformTerminalWorkerBackend<Dispatch>
where
    Dispatch: IRuntimeEventDispatcher,
{
    pub fn new(proxy: Dispatch, scrollback_history: usize) -> Self {
        Self {
            proxy,
            scrollback_history,
            pool: OnceLock::new(),
        }
    }

    #[cfg(test)]
    fn with_worker_count(proxy: Dispatch, worker_count: usize, scrollback_history: usize) -> Self {
        let pool = OnceLock::new();
        let _ = pool.set(TerminalWorkerPool::new(worker_count));
        Self {
            proxy,
            scrollback_history,
            pool,
        }
    }

    fn pool(&self) -> &TerminalWorkerPool<Dispatch> {
        self.pool
            .get_or_init(|| TerminalWorkerPool::new(terminal_worker_pool_size()))
    }
}

impl<Dispatch> ITerminalWorkerBackend for PlatformTerminalWorkerBackend<Dispatch>
where
    Dispatch: IRuntimeEventDispatcher,
{
    fn start_worker_pool(&self) {
        let _ = self.pool();
    }

    fn spawn_terminal_worker(
        &self,
        gshell_id: GShellId,
        initial_size: TerminalGridSize,
        surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
        snapshot_wake_pending: Arc<AtomicBool>,
    ) -> SyncSender<TerminalWorkerInput> {
        self.pool().spawn_terminal_worker(
            self.proxy.clone(),
            gshell_id,
            initial_size,
            self.scrollback_history,
            surface_snapshot_tx,
            snapshot_wake_pending,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Sender},
    };

    use germinal_domain::{
        gshell::vo::gshell_id::GShellId, pty_host::terminal_size::TerminalGridSize,
    };
    use germinal_ports::{
        event::{
            runtime_event::{GShellRuntimeEvent, RuntimeEvent},
            runtime_event_dispatcher::IRuntimeEventDispatcher,
        },
        pty_host::{
            pty_input::pty_input_channel,
            terminal_input_mode::TerminalInputModeState,
            worker_backend::ITerminalWorkerBackend,
            worker_input::{
                TerminalDisplayScroll, TerminalSelectionKind, TerminalSelectionPoint,
                TerminalSelectionSide, TerminalWorkerInput,
            },
        },
        rendering::surface_snapshot::RenderSurfaceSnapshot,
    };

    use super::{PlatformTerminalWorkerBackend, TerminalWorkerRuntime};

    const TEST_SCROLLBACK_HISTORY: usize = 10_000;

    #[derive(Clone)]
    struct TestDispatcher {
        tx: Sender<RuntimeEvent>,
    }

    impl IRuntimeEventDispatcher for TestDispatcher {
        fn dispatch(
            &self,
            event: RuntimeEvent,
        ) -> Result<(), germinal_ports::event::runtime_event_dispatcher::RuntimeEventDispatchError>
        {
            self.tx.send(event).map_err(|_| {
                germinal_ports::event::runtime_event_dispatcher::RuntimeEventDispatchError::Closed
            })?;
            Ok(())
        }
    }

    #[test]
    fn pooled_backend_handles_multiple_terminals_on_one_lane() {
        let (event_tx, event_rx) = mpsc::channel::<RuntimeEvent>();
        let backend = PlatformTerminalWorkerBackend::with_worker_count(
            TestDispatcher { tx: event_tx },
            1,
            TEST_SCROLLBACK_HISTORY,
        );
        let (snapshot_tx, snapshot_rx) = mpsc::channel::<RenderSurfaceSnapshot>();

        let first_input = backend.spawn_terminal_worker(
            GShellId::new(1),
            TerminalGridSize::new(80, 24),
            snapshot_tx.clone(),
            Arc::new(AtomicBool::new(false)),
        );
        let second_input = backend.spawn_terminal_worker(
            GShellId::new(2),
            TerminalGridSize::new(80, 24),
            snapshot_tx,
            Arc::new(AtomicBool::new(false)),
        );

        first_input
            .send(
                germinal_ports::pty_host::worker_input::TerminalWorkerInput::Bytes(
                    b"first".to_vec(),
                ),
            )
            .expect("first terminal input should send");
        second_input
            .send(
                germinal_ports::pty_host::worker_input::TerminalWorkerInput::Bytes(
                    b"second".to_vec(),
                ),
            )
            .expect("second terminal input should send");

        let first_snapshot = snapshot_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("first snapshot should arrive");
        let second_snapshot = snapshot_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("second snapshot should arrive");

        let mut snapshot_targets = vec![
            first_snapshot.target_id.value(),
            second_snapshot.target_id.value(),
        ];
        snapshot_targets.sort_unstable();
        assert_eq!(snapshot_targets, vec![1, 2]);

        let first_event = event_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("first frame-ready event should arrive");
        let second_event = event_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("second frame-ready event should arrive");

        let mut event_targets = vec![gshell_id_of(first_event), gshell_id_of(second_event)];
        event_targets.sort_unstable();
        assert_eq!(event_targets, vec![1, 2]);
    }

    #[test]
    fn terminal_worker_strips_enter_gnative_control_sequence_and_dispatches_mode_switch() {
        let (event_tx, event_rx) = mpsc::channel::<RuntimeEvent>();
        let backend = PlatformTerminalWorkerBackend::with_worker_count(
            TestDispatcher { tx: event_tx },
            1,
            TEST_SCROLLBACK_HISTORY,
        );
        let (snapshot_tx, snapshot_rx) = mpsc::channel::<RenderSurfaceSnapshot>();
        let input = backend.spawn_terminal_worker(
            GShellId::new(3),
            TerminalGridSize::new(80, 24),
            snapshot_tx,
            Arc::new(AtomicBool::new(false)),
        );

        input
            .send(
                germinal_ports::pty_host::worker_input::TerminalWorkerInput::Bytes(
                    b"left\x1bPgerminal-gnative;\x1b\\right".to_vec(),
                ),
            )
            .expect("terminal input should send");

        let snapshot = snapshot_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("surface snapshot should arrive");
        let rendered_text: String = snapshot
            .rows
            .iter()
            .flat_map(|row| row.runs.iter())
            .map(|run| run.text.as_str())
            .collect();
        assert_eq!(rendered_text.trim_end(), "leftright");

        let first_event = event_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("enter-gnative event should arrive");
        assert_eq!(
            first_event,
            RuntimeEvent::GShell(GShellRuntimeEvent::EnterGNative {
                gshell_id: GShellId::new(3)
            })
        );

        let second_event = event_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("frame-ready event should arrive");
        assert!(matches!(
            second_event,
            RuntimeEvent::GShell(GShellRuntimeEvent::FrameReady {
                gshell_id,
                ..
            }) if gshell_id == GShellId::new(3)
        ));
    }

    #[test]
    fn terminal_worker_publishes_parsed_input_modes() {
        let (event_tx, _event_rx) = mpsc::channel::<RuntimeEvent>();
        let backend = PlatformTerminalWorkerBackend::with_worker_count(
            TestDispatcher { tx: event_tx },
            1,
            TEST_SCROLLBACK_HISTORY,
        );
        let (snapshot_tx, snapshot_rx) = mpsc::channel::<RenderSurfaceSnapshot>();
        let input = backend.spawn_terminal_worker(
            GShellId::new(4),
            TerminalGridSize::new(80, 24),
            snapshot_tx,
            Arc::new(AtomicBool::new(false)),
        );
        let (pty_tx, _pty_rx) = pty_input_channel();
        let input_modes = TerminalInputModeState::default();
        input
            .send(TerminalWorkerInput::SetPtyInput {
                sender: pty_tx,
                input_modes: input_modes.clone(),
            })
            .expect("PTY input state should send");
        input
            .send(TerminalWorkerInput::Bytes(
                b"\x1b[?1h\x1b[?2004h\x1b[?1004h\x1b[?1000h\x1b[?1006h".to_vec(),
            ))
            .expect("terminal mode sequences should send");

        snapshot_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("surface snapshot should arrive after mode update");
        let modes = input_modes.load();
        assert!(modes.app_cursor());
        assert!(modes.bracketed_paste());
        assert!(modes.focus_in_out());
        assert!(modes.sgr_mouse());
        assert!(modes.mouse_report_click());
    }

    #[test]
    fn terminal_worker_publishes_scrolled_history() {
        let (event_tx, event_rx) = mpsc::channel::<RuntimeEvent>();
        let backend = PlatformTerminalWorkerBackend::with_worker_count(
            TestDispatcher { tx: event_tx },
            1,
            TEST_SCROLLBACK_HISTORY,
        );
        let (snapshot_tx, snapshot_rx) = mpsc::channel::<RenderSurfaceSnapshot>();
        let wake_pending = Arc::new(AtomicBool::new(false));
        let input = backend.spawn_terminal_worker(
            GShellId::new(6),
            TerminalGridSize::new(8, 2),
            snapshot_tx,
            wake_pending.clone(),
        );

        input
            .send(TerminalWorkerInput::Bytes(b"one\r\ntwo\r\nthree".to_vec()))
            .expect("terminal output should send");
        snapshot_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("live snapshot should arrive");
        event_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("live frame-ready event should arrive");
        wake_pending.store(false, Ordering::Release);

        input
            .send(TerminalWorkerInput::ScrollDisplay(
                TerminalDisplayScroll::Delta(1),
            ))
            .expect("scroll command should send");
        let snapshot = snapshot_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("history snapshot should arrive");
        let text: String = snapshot
            .rows
            .iter()
            .flat_map(|row| &row.runs)
            .map(|run| run.text.as_str())
            .collect();
        assert!(text.contains("one"));
        assert!(!text.contains("three"));
        assert!(snapshot.cursor.is_none());
    }

    #[test]
    fn terminal_worker_returns_selected_text_to_the_app() {
        let (event_tx, event_rx) = mpsc::channel::<RuntimeEvent>();
        let backend = PlatformTerminalWorkerBackend::with_worker_count(
            TestDispatcher { tx: event_tx },
            1,
            TEST_SCROLLBACK_HISTORY,
        );
        let (snapshot_tx, snapshot_rx) = mpsc::channel::<RenderSurfaceSnapshot>();
        let wake_pending = Arc::new(AtomicBool::new(false));
        let input = backend.spawn_terminal_worker(
            GShellId::new(7),
            TerminalGridSize::new(16, 2),
            snapshot_tx,
            wake_pending.clone(),
        );

        input
            .send(TerminalWorkerInput::Bytes(b"hello world".to_vec()))
            .expect("terminal output should send");
        snapshot_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("initial snapshot should arrive");
        event_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("initial frame-ready event should arrive");
        wake_pending.store(false, Ordering::Release);

        input
            .send(TerminalWorkerInput::RequestSelectionText)
            .expect("empty selection text request should send");
        assert_eq!(
            event_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .expect("empty selection text event should arrive"),
            RuntimeEvent::GShell(GShellRuntimeEvent::SelectionText {
                gshell_id: GShellId::new(7),
                text: None,
            })
        );

        input
            .send(TerminalWorkerInput::StartSelection {
                kind: TerminalSelectionKind::Character,
                point: TerminalSelectionPoint::new(0, 0, TerminalSelectionSide::Left),
            })
            .expect("selection start should send");
        input
            .send(TerminalWorkerInput::UpdateSelection(
                TerminalSelectionPoint::new(4, 0, TerminalSelectionSide::Right),
            ))
            .expect("selection update should send");
        input
            .send(TerminalWorkerInput::RequestSelectionText)
            .expect("selection text request should send");

        assert_eq!(
            event_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .expect("selection text event should arrive"),
            RuntimeEvent::GShell(GShellRuntimeEvent::SelectionText {
                gshell_id: GShellId::new(7),
                text: Some("hello".to_string()),
            })
        );
    }

    #[test]
    fn terminal_worker_does_not_publish_mid_synchronized_update() {
        let (event_tx, _event_rx) = mpsc::channel::<RuntimeEvent>();
        let (snapshot_tx, snapshot_rx) = mpsc::channel::<RenderSurfaceSnapshot>();
        let wake_pending = Arc::new(AtomicBool::new(false));
        let mut runtime = TerminalWorkerRuntime::new(
            TestDispatcher { tx: event_tx },
            GShellId::new(5),
            super::AlacrittyTermSize::new(20, 4),
            TEST_SCROLLBACK_HISTORY,
            snapshot_tx,
            wake_pending.clone(),
        );

        let initial_seq = runtime.apply_byte_chunks(&[b"old".to_vec()]).unwrap();
        runtime.unpublished_seq = Some(initial_seq);
        assert!(runtime.publish_unpublished_snapshot());
        snapshot_rx
            .try_recv()
            .expect("initial snapshot should publish");
        wake_pending.store(false, Ordering::Release);

        let pending_seq = runtime
            .apply_byte_chunks(&[b"\x1b[?2026h\x1b[2J\x1b[Hreplacement".to_vec()])
            .unwrap();
        runtime.unpublished_seq = Some(pending_seq);
        assert!(!runtime.publish_unpublished_snapshot());
        assert!(snapshot_rx.try_recv().is_err());

        let completed_seq = runtime
            .apply_byte_chunks(&[b"\x1b[?2026l".to_vec()])
            .unwrap();
        runtime.unpublished_seq = Some(completed_seq);
        assert!(runtime.publish_unpublished_snapshot());
        let snapshot = snapshot_rx
            .try_recv()
            .expect("completed snapshot should publish");
        let text: String = snapshot
            .rows
            .iter()
            .flat_map(|row| &row.runs)
            .map(|run| run.text.as_str())
            .collect();
        assert!(text.contains("replacement"));
    }

    fn gshell_id_of(event: RuntimeEvent) -> u64 {
        match event {
            RuntimeEvent::GShell(GShellRuntimeEvent::FrameReady { gshell_id, .. }) => {
                gshell_id.value()
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }
}
