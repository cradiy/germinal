use std::{
	env,
	sync::{
		Arc,
		atomic::{AtomicBool, Ordering},
		mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError},
	},
	thread,
	time::{Duration, Instant},
};

use germinal_domain::{gshell::vo::gshell_id::GShellId, pty_host::terminal_size::TerminalGridSize};
use germinal_ports::{
	event::{
		runtime_event::{GShellRuntimeEvent, RuntimeEvent},
		runtime_event_dispatcher::RuntimeEventDispatcher,
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

use crate::pty_host::alacritty_terminal_store::{AlacrittyTermSize, AlacrittyTerminalStore};

const TERMINAL_INPUT_CHANNEL_CAPACITY: usize = 64;
const MAX_PENDING_BYTES_BEFORE_APPLY: usize = 256 * 1024;
const MAX_EVENTS_PER_WORKER_TICK: usize = 256;
const PUBLISH_RETRY_INTERVAL: Duration = Duration::from_millis(2);
const PERF_LOG_INTERVAL: Duration = Duration::from_secs(1);
const TERMINAL_WORKER_PERF_LOG_ENV: &str = "GERMINAL_TERMINAL_WORKER_PERF_LOG";

pub struct TerminalWorker;

impl TerminalWorker {
	pub fn spawn(
		proxy: RuntimeEventDispatcher,
		gshell_id: GShellId,
		initial_size: TerminalGridSize,
		surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
		snapshot_wake_pending: Arc<AtomicBool>,
	) -> SyncSender<TerminalWorkerInput> {
		let (tx, rx) = mpsc::sync_channel::<TerminalWorkerInput>(TERMINAL_INPUT_CHANNEL_CAPACITY);

		thread::spawn(move || {
			let mut runtime = TerminalWorkerRuntime::new(
				proxy,
				gshell_id,
				to_alacritty_term_size(initial_size),
				surface_snapshot_tx,
				snapshot_wake_pending,
			);

			runtime.run(rx);
		});

		tx
	}
}

struct TerminalWorkerRuntime {
	proxy: RuntimeEventDispatcher,

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

	pty_input_tx: Option<PtyInputSender>,
}

impl TerminalWorkerRuntime {
	fn new(
		proxy: RuntimeEventDispatcher,
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
		}
	}

	fn run(&mut self, rx: Receiver<TerminalWorkerInput>) {
		loop {
			let next_input = if self.unpublished_seq.is_some() {
				match rx.recv_timeout(PUBLISH_RETRY_INTERVAL) {
					Ok(input) => Some(input),
					Err(RecvTimeoutError::Timeout) => None,
					Err(RecvTimeoutError::Disconnected) => break,
				}
			} else {
				match rx.recv() {
					Ok(input) => Some(input),
					Err(_) => break,
				}
			};

			if let Some(first_input) = next_input {
				self.process_batch(first_input, &rx);

				if self.pending_bytes_len >= MAX_PENDING_BYTES_BEFORE_APPLY {
					self.flush_pending_input();
				}

				self.flush_pending_input();
			}

			self.publish_unpublished_snapshot();
			self.perf.maybe_log();
		}

		self.flush_pending_input();
		self.publish_unpublished_snapshot();
		self.perf.maybe_force_log();
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

		self.unpublished_seq = Some(self.apply_byte_chunks(&chunks));
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

	fn apply_byte_chunks(&mut self, chunks: &[Vec<u8>]) -> Seq {
		self.seq += 1;

		let seq = Seq::new(self.seq);
		let started_at = Instant::now();

		for bytes in chunks {
			self.terminal_store.apply_bytes(self.target_id, seq, bytes);

			let pending_pty_writes = self.terminal_store.take_pending_pty_writes(self.target_id);
			self.forward_pty_writes(pending_pty_writes);
		}

		let elapsed = started_at.elapsed();

		self.perf.apply_batches += 1;
		self.perf.apply_chunks += chunks.len() as u64;
		self.perf.apply_time += elapsed;
		self.perf.apply_max = self.perf.apply_max.max(elapsed);

		seq
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

		eprintln!(
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

fn to_alacritty_term_size(size: TerminalGridSize) -> AlacrittyTermSize {
	AlacrittyTermSize::new(size.columns(), size.rows())
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PlatformTerminalWorkerBackend;

impl PlatformTerminalWorkerBackend {
	pub fn new() -> Self { Self }
}

impl ITerminalWorkerBackend for PlatformTerminalWorkerBackend {
	fn spawn_terminal_worker(
		&self,
		gshell_id: GShellId,
		initial_size: TerminalGridSize,
		proxy: RuntimeEventDispatcher,
		surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
		snapshot_wake_pending: Arc<AtomicBool>,
	) -> SyncSender<TerminalWorkerInput> {
		TerminalWorker::spawn(
			proxy,
			gshell_id,
			initial_size,
			surface_snapshot_tx,
			snapshot_wake_pending,
		)
	}
}
