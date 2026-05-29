use std::{
	collections::VecDeque,
	sync::{Arc, Condvar, Mutex, mpsc::SyncSender},
	task::{Poll, Waker},
};

use germinal_domain::{pty_host::terminal_size::TerminalPtySize, workspace::pane_id::PaneId};
use germinal_ports::event::runtime_event_dispatcher::RuntimeEventDispatcher;
use portable_pty::PtySize;

#[cfg(unix)]
use crate::pty::unix_pty_bridge::spawn_compio_bridge_thread;
#[cfg(windows)]
use crate::pty::windows_pty_bridge::spawn_blocking_bridge_thread;
use crate::{
	pty::shell_command::{ShellCommand, default_shell_command},
	pty_host::worker::TerminalWorkerInput,
};

#[derive(Debug)]
pub enum PtyBridgeInput {
	Bytes(Vec<u8>),
	Resize(TerminalPtySize),
}

#[derive(Debug)]
struct PtyInputQueueState {
	queue:           VecDeque<PtyBridgeInput>,
	sender_count:    usize,
	receiver_closed: bool,
	waker:           Option<Waker>,
}

#[derive(Debug)]
struct PtyInputQueue {
	state:     Mutex<PtyInputQueueState>,
	available: Condvar,
}

#[derive(Debug)]
pub struct PtyInputSender {
	queue: Arc<PtyInputQueue>,
}

#[derive(Debug)]
pub(crate) struct PtyInputReceiver {
	queue: Arc<PtyInputQueue>,
}

impl Clone for PtyInputSender {
	fn clone(&self) -> Self {
		let mut state = self.queue.state.lock().expect("pty input queue mutex poisoned");
		state.sender_count += 1;
		drop(state);

		Self { queue: Arc::clone(&self.queue) }
	}
}

impl Drop for PtyInputSender {
	fn drop(&mut self) {
		let mut state = self.queue.state.lock().expect("pty input queue mutex poisoned");
		if state.sender_count > 0 {
			state.sender_count -= 1;
		}
		let should_wake = state.sender_count == 0;
		let waker = if should_wake { state.waker.take() } else { None };
		drop(state);

		if should_wake {
			self.queue.available.notify_all();
			if let Some(waker) = waker {
				waker.wake();
			}
		}
	}
}

impl PtyInputSender {
	pub fn send(&self, input: PtyBridgeInput) -> Result<(), PtyBridgeInput> {
		let mut state = self.queue.state.lock().expect("pty input queue mutex poisoned");
		if state.receiver_closed {
			return Err(input);
		}

		state.queue.push_back(input);
		let waker = state.waker.take();
		drop(state);

		self.queue.available.notify_one();
		if let Some(waker) = waker {
			waker.wake();
		}

		Ok(())
	}

	pub fn close(&self) {
		let mut state = self.queue.state.lock().expect("pty input queue mutex poisoned");
		if state.receiver_closed {
			return;
		}

		state.receiver_closed = true;
		let waker = state.waker.take();
		drop(state);

		self.queue.available.notify_all();
		if let Some(waker) = waker {
			waker.wake();
		}
	}
}

impl PtyInputReceiver {
	pub(crate) async fn recv(&self) -> Option<PtyBridgeInput> {
		std::future::poll_fn(|cx| {
			let mut state = self.queue.state.lock().expect("pty input queue mutex poisoned");

			if let Some(input) = state.queue.pop_front() {
				return Poll::Ready(Some(input));
			}

			if state.receiver_closed || state.sender_count == 0 {
				return Poll::Ready(None);
			}

			state.waker = Some(cx.waker().clone());
			Poll::Pending
		})
		.await
	}

	#[cfg(windows)]
	pub(crate) fn recv_blocking(&self) -> Option<PtyBridgeInput> {
		let mut state = self.queue.state.lock().expect("pty input queue mutex poisoned");

		loop {
			if let Some(input) = state.queue.pop_front() {
				return Some(input);
			}

			if state.receiver_closed || state.sender_count == 0 {
				return None;
			}

			state =
				self.queue.available.wait(state).expect("pty input queue mutex poisoned while waiting");
		}
	}
}

fn pty_input_channel() -> (PtyInputSender, PtyInputReceiver) {
	let queue = Arc::new(PtyInputQueue {
		state:     Mutex::new(PtyInputQueueState {
			queue:           VecDeque::new(),
			sender_count:    1,
			receiver_closed: false,
			waker:           None,
		}),
		available: Condvar::new(),
	});

	(PtyInputSender { queue: Arc::clone(&queue) }, PtyInputReceiver { queue })
}

#[derive(Debug, Clone)]
pub(crate) struct PtyBridgeConfig {
	pub shell:        ShellCommand,
	pub initial_size: TerminalPtySize,
}

impl PtyBridgeConfig {
	pub fn new(initial_size: TerminalPtySize) -> Self {
		Self { shell: default_shell_command(), initial_size }
	}
}

pub struct PtyBridge;

impl PtyBridge {
	pub fn spawn(
		proxy: RuntimeEventDispatcher,
		pane_id: PaneId,
		initial_size: TerminalPtySize,
		terminal_worker_tx: SyncSender<TerminalWorkerInput>,
	) -> PtyInputSender {
		Self::spawn_with_config(proxy, pane_id, PtyBridgeConfig::new(initial_size), terminal_worker_tx)
	}

	pub(crate) fn spawn_with_config(
		proxy: RuntimeEventDispatcher,
		pane_id: PaneId,
		config: PtyBridgeConfig,
		terminal_worker_tx: SyncSender<TerminalWorkerInput>,
	) -> PtyInputSender {
		let (input_tx, input_rx) = pty_input_channel();

		#[cfg(unix)]
		spawn_compio_bridge_thread(
			proxy,
			pane_id,
			config,
			terminal_worker_tx,
			input_tx.clone(),
			input_rx,
		);

		#[cfg(windows)]
		spawn_blocking_bridge_thread(proxy, pane_id, config, terminal_worker_tx, input_rx);

		input_tx
	}
}

pub(crate) fn to_portable_pty_size(size: TerminalPtySize) -> PtySize {
	PtySize {
		rows:         size.rows(),
		cols:         size.columns(),
		pixel_width:  size.pixel_width(),
		pixel_height: size.pixel_height(),
	}
}
