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
		worker_backend::ITerminalWorkerBackend,
		worker_input::TerminalWorkerInput,
	},
	rendering::{
		render_target_id::RenderTargetId,
		surface_snapshot::{RenderSurfaceCursorSnapshot, RenderSurfaceSnapshot},
	},
	seq::Seq,
};
use rayon::ThreadPool;
use tracing::info;

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
	proxy:                 Dispatch,
	gshell_id:             GShellId,
	initial_size:          TerminalGridSize,
	surface_snapshot_tx:   Sender<RenderSurfaceSnapshot>,
	snapshot_wake_pending: Arc<AtomicBool>,
	input_rx:              Receiver<TerminalWorkerInput>,
}

struct TerminalWorkerHandle<Dispatch> {
	runtime:  TerminalWorkerRuntime<Dispatch>,
	input_rx: Receiver<TerminalWorkerInput>,
}

enum TerminalWorkerTick {
	Idle,
	Progressed,
	Finished,
}

impl<Dispatch> TerminalWorkerHandle<Dispatch>
where Dispatch: IRuntimeEventDispatcher
{
	fn new(registration: TerminalWorkerRegistration<Dispatch>) -> Self {
		Self {
			runtime:  TerminalWorkerRuntime::new(
				registration.proxy,
				registration.gshell_id,
				to_alacritty_term_size(registration.initial_size),
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

		let had_unpublished = self.runtime.unpublished_seq.is_some();
		self.runtime.publish_unpublished_snapshot();
		progressed |= had_unpublished || self.runtime.unpublished_seq.is_some();

		if disconnected {
			self.runtime.flush_pending_input();
			self.runtime.publish_unpublished_snapshot();

			if self.runtime.unpublished_seq.is_none() {
				self.runtime.perf.maybe_force_log();
				return TerminalWorkerTick::Finished;
			}

			progressed = true;
		}

		self.runtime.perf.maybe_log();

		if progressed { TerminalWorkerTick::Progressed } else { TerminalWorkerTick::Idle }
	}
}

struct TerminalWorkerPool<Dispatch> {
	_thread_pool:     ThreadPool,
	registration_txs: Vec<Sender<TerminalWorkerRegistration<Dispatch>>>,
	next_lane:        AtomicUsize,
}

impl<Dispatch> TerminalWorkerPool<Dispatch>
where Dispatch: IRuntimeEventDispatcher
{
	fn new(worker_count: usize) -> Self {
		let worker_count = worker_count.max(1);
		let thread_pool = rayon::ThreadPoolBuilder::new()
			.num_threads(worker_count)
			.thread_name(|lane_index| format!("terminal-worker-{lane_index}"))
			.build()
			.expect("failed to build terminal worker thread pool");
		let mut registration_txs = Vec::with_capacity(worker_count);

		for _lane_index in 0..worker_count {
			let (registration_tx, registration_rx) =
				mpsc::channel::<TerminalWorkerRegistration<Dispatch>>();

			thread_pool.spawn_fifo(move || run_terminal_worker_lane(registration_rx));

			registration_txs.push(registration_tx);
		}

		Self { _thread_pool: thread_pool, registration_txs, next_lane: AtomicUsize::new(0) }
	}

	fn spawn_terminal_worker(
		&self,
		proxy: Dispatch,
		gshell_id: GShellId,
		initial_size: TerminalGridSize,
		surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
		snapshot_wake_pending: Arc<AtomicBool>,
	) -> SyncSender<TerminalWorkerInput> {
		let (tx, rx) = mpsc::sync_channel::<TerminalWorkerInput>(TERMINAL_INPUT_CHANNEL_CAPACITY);
		let lane_index = self.next_lane.fetch_add(1, Ordering::Relaxed) % self.registration_txs.len();
		let registration = TerminalWorkerRegistration {
			proxy,
			gshell_id,
			initial_size,
			surface_snapshot_tx,
			snapshot_wake_pending,
			input_rx: rx,
		};

		self.registration_txs[lane_index]
			.send(registration)
			.expect("terminal worker lane should accept registrations");

		tx
	}
}

fn run_terminal_worker_lane<Dispatch>(
	registration_rx: Receiver<TerminalWorkerRegistration<Dispatch>>,
) where Dispatch: IRuntimeEventDispatcher {
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
	seq:       u64,

	terminal_store: AlacrittyTerminalStore,

	surface_snapshot_tx:   Sender<RenderSurfaceSnapshot>,
	snapshot_wake_pending: Arc<AtomicBool>,

	pending_chunks:    Vec<Vec<u8>>,
	pending_bytes_len: usize,

	unpublished_seq: Option<Seq>,

	perf: TerminalWorkerPerf,

	pty_input_tx:          Option<PtyInputSender>,
	gnative_enter_decoder: GNativeEnterControlSequenceDecoder,
}

impl<Dispatch> TerminalWorkerRuntime<Dispatch>
where Dispatch: IRuntimeEventDispatcher
{
	fn new(
		proxy: Dispatch,
		gshell_id: GShellId,
		initial_size: AlacrittyTermSize,
		surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
		snapshot_wake_pending: Arc<AtomicBool>,
	) -> Self {
		let target_id = RenderTargetId::new(gshell_id.value());
		let terminal_store = AlacrittyTerminalStore::with_size(initial_size);

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
			TerminalWorkerInput::SetPtyInput(tx) => {
				self.pty_input_tx = Some(tx);
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
			self.terminal_store.apply_bytes(self.target_id, seq, &decode_result.visible_bytes);

			let pending_pty_writes = self.terminal_store.take_pending_pty_writes(self.target_id);
			self.forward_pty_writes(pending_pty_writes);
		}

		if enter_gnative {
			let _ = self.proxy.dispatch(RuntimeEvent::GShell(GShellRuntimeEvent::EnterGNative {
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

	fn publish_unpublished_snapshot(&mut self) {
		if self.unpublished_seq.is_none() {
			return;
		}

		if self.snapshot_wake_pending.load(Ordering::Acquire) {
			self.perf.coalesced_wakeups += 1;
			return;
		}

		let Some(seq) = self.unpublished_seq.take() else {
			return;
		};

		let started_at = Instant::now();
		let snapshot_started_at = Instant::now();
		let mut snapshot = self
			.terminal_store
			.render_surface_snapshot_of(self.target_id)
			.expect("surface snapshot should exist");
		self.perf.publish_snapshot += snapshot_started_at.elapsed();

		let cursor_started_at = Instant::now();
		snapshot.cursor = self
			.terminal_store
			.cursor_position_0_based(self.target_id)
			.map(|(x, y)| RenderSurfaceCursorSnapshot { x, y, focused: true });
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
			return;
		}

		if wake_already_pending {
			self.perf.coalesced_wakeups += 1;
			self.unpublished_seq = Some(seq);
			return;
		}

		let dispatch_started_at = Instant::now();
		let _ = self.proxy.dispatch(RuntimeEvent::GShell(GShellRuntimeEvent::FrameReady {
			gshell_id: self.gshell_id,
			seq,
		}));
		self.perf.publish_dispatch += dispatch_started_at.elapsed();
	}
}

struct TerminalWorkerPerf {
	logging_enabled: bool,
	started_at:      Instant,
	last_log_at:     Instant,

	input_chunks: u64,
	input_bytes:  u64,

	apply_batches: u64,
	apply_chunks:  u64,
	apply_time:    Duration,
	apply_max:     Duration,

	publish_count:    u64,
	publish_time:     Duration,
	publish_max:      Duration,
	publish_snapshot: Duration,
	publish_cursor:   Duration,
	publish_send:     Duration,
	publish_clear:    Duration,
	publish_dispatch: Duration,

	resize_count: u64,
	resize_time:  Duration,
	resize_max:   Duration,

	coalesced_wakeups: u64,
}

impl TerminalWorkerPerf {
	fn new() -> Self {
		let now = Instant::now();

		Self {
			logging_enabled: terminal_worker_perf_logging_enabled(),
			started_at:      now,
			last_log_at:     now,

			input_chunks: 0,
			input_bytes:  0,

			apply_batches: 0,
			apply_chunks:  0,
			apply_time:    Duration::ZERO,
			apply_max:     Duration::ZERO,

			publish_count:    0,
			publish_time:     Duration::ZERO,
			publish_max:      Duration::ZERO,
			publish_snapshot: Duration::ZERO,
			publish_cursor:   Duration::ZERO,
			publish_send:     Duration::ZERO,
			publish_clear:    Duration::ZERO,
			publish_dispatch: Duration::ZERO,

			resize_count: 0,
			resize_time:  Duration::ZERO,
			resize_max:   Duration::ZERO,

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
	env::var_os(TERMINAL_WORKER_PERF_LOG_ENV).and_then(|value| value.into_string().ok()).is_some_and(
		|value| matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"),
	)
}

fn terminal_worker_pool_size() -> usize {
	let env_size = env::var_os(TERMINAL_WORKER_POOL_ENV)
		.and_then(|value| value.into_string().ok())
		.and_then(|value| value.trim().parse::<usize>().ok())
		.filter(|&value| value > 0);

	env_size.or_else(|| thread::available_parallelism().map(NonZeroUsize::get).ok()).unwrap_or(1)
}

fn to_alacritty_term_size(size: TerminalGridSize) -> AlacrittyTermSize {
	AlacrittyTermSize::new(size.columns(), size.rows())
}

pub struct PlatformTerminalWorkerBackend<Dispatch> {
	proxy: Dispatch,
	pool:  OnceLock<TerminalWorkerPool<Dispatch>>,
}

impl<Dispatch> PlatformTerminalWorkerBackend<Dispatch>
where Dispatch: IRuntimeEventDispatcher
{
	pub fn new(proxy: Dispatch) -> Self { Self { proxy, pool: OnceLock::new() } }

	#[cfg(test)]
	fn with_worker_count(proxy: Dispatch, worker_count: usize) -> Self {
		let pool = OnceLock::new();
		let _ = pool.set(TerminalWorkerPool::new(worker_count));
		Self { proxy, pool }
	}

	fn pool(&self) -> &TerminalWorkerPool<Dispatch> {
		self.pool.get_or_init(|| TerminalWorkerPool::new(terminal_worker_pool_size()))
	}
}

impl<Dispatch> ITerminalWorkerBackend for PlatformTerminalWorkerBackend<Dispatch>
where Dispatch: IRuntimeEventDispatcher
{
	fn start_worker_pool(&self) { let _ = self.pool(); }

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
			surface_snapshot_tx,
			snapshot_wake_pending,
		)
	}
}

#[cfg(test)]
mod tests {
	use std::sync::{
		Arc,
		atomic::AtomicBool,
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
		pty_host::worker_backend::ITerminalWorkerBackend,
		rendering::surface_snapshot::RenderSurfaceSnapshot,
	};

	use super::PlatformTerminalWorkerBackend;

	#[derive(Clone)]
	struct TestDispatcher {
		tx: Sender<RuntimeEvent>,
	}

	impl IRuntimeEventDispatcher for TestDispatcher {
		fn dispatch(&self, event: RuntimeEvent) -> germinal_ports::error::BoxResult<()> {
			self.tx.send(event)?;
			Ok(())
		}
	}

	#[test]
	fn pooled_backend_handles_multiple_terminals_on_one_lane() {
		let (event_tx, event_rx) = mpsc::channel::<RuntimeEvent>();
		let backend =
			PlatformTerminalWorkerBackend::with_worker_count(TestDispatcher { tx: event_tx }, 1);
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
			.send(germinal_ports::pty_host::worker_input::TerminalWorkerInput::Bytes(b"first".to_vec()))
			.expect("first terminal input should send");
		second_input
			.send(germinal_ports::pty_host::worker_input::TerminalWorkerInput::Bytes(b"second".to_vec()))
			.expect("second terminal input should send");

		let first_snapshot = snapshot_rx
			.recv_timeout(std::time::Duration::from_secs(1))
			.expect("first snapshot should arrive");
		let second_snapshot = snapshot_rx
			.recv_timeout(std::time::Duration::from_secs(1))
			.expect("second snapshot should arrive");

		let mut snapshot_targets =
			vec![first_snapshot.target_id.value(), second_snapshot.target_id.value()];
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
		let backend =
			PlatformTerminalWorkerBackend::with_worker_count(TestDispatcher { tx: event_tx }, 1);
		let (snapshot_tx, snapshot_rx) = mpsc::channel::<RenderSurfaceSnapshot>();
		let input = backend.spawn_terminal_worker(
			GShellId::new(3),
			TerminalGridSize::new(80, 24),
			snapshot_tx,
			Arc::new(AtomicBool::new(false)),
		);

		input
			.send(germinal_ports::pty_host::worker_input::TerminalWorkerInput::Bytes(
				b"left\x1bPgerminal-gnative;\x1b\\right".to_vec(),
			))
			.expect("terminal input should send");

		let snapshot = snapshot_rx
			.recv_timeout(std::time::Duration::from_secs(1))
			.expect("surface snapshot should arrive");
		let rendered_text: String =
			snapshot.rows.iter().flat_map(|row| row.runs.iter()).map(|run| run.text.as_str()).collect();
		assert_eq!(rendered_text.trim_end(), "leftright");

		let first_event = event_rx
			.recv_timeout(std::time::Duration::from_secs(1))
			.expect("enter-gnative event should arrive");
		assert_eq!(
			first_event,
			RuntimeEvent::GShell(GShellRuntimeEvent::EnterGNative { gshell_id: GShellId::new(3) })
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

	fn gshell_id_of(event: RuntimeEvent) -> u64 {
		match event {
			RuntimeEvent::GShell(GShellRuntimeEvent::FrameReady { gshell_id, .. }) => gshell_id.value(),
			other => panic!("unexpected event: {other:?}"),
		}
	}
}
