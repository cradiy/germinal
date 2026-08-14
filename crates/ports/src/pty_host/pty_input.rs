use std::{
    collections::VecDeque,
    sync::{Arc, Condvar, Mutex},
    task::{Poll, Waker},
};

use crate::pty_host::terminal_size::TerminalPtySize;

#[derive(Debug)]
pub enum PtyInput {
    Bytes(Vec<u8>),
    Resize(TerminalPtySize),
}

#[derive(Debug)]
struct PtyInputQueueState {
    queue: VecDeque<PtyInput>,
    sender_count: usize,
    receiver_closed: bool,
    waker: Option<Waker>,
}

#[derive(Debug)]
struct PtyInputQueue {
    state: Mutex<PtyInputQueueState>,
    available: Condvar,
}

#[derive(Debug)]
pub struct PtyInputSender {
    queue: Arc<PtyInputQueue>,
}

#[derive(Debug)]
pub struct PtyInputReceiver {
    queue: Arc<PtyInputQueue>,
}

impl Clone for PtyInputSender {
    fn clone(&self) -> Self {
        let mut state = self
            .queue
            .state
            .lock()
            .expect("pty input queue mutex poisoned");
        state.sender_count += 1;
        drop(state);

        Self {
            queue: Arc::clone(&self.queue),
        }
    }
}

impl Drop for PtyInputSender {
    fn drop(&mut self) {
        let mut state = self
            .queue
            .state
            .lock()
            .expect("pty input queue mutex poisoned");
        if state.sender_count > 0 {
            state.sender_count -= 1;
        }
        let should_wake = state.sender_count == 0;
        let waker = if should_wake {
            state.waker.take()
        } else {
            None
        };
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
    pub fn send(&self, input: PtyInput) -> Result<(), PtyInput> {
        let mut state = self
            .queue
            .state
            .lock()
            .expect("pty input queue mutex poisoned");
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
        let mut state = self
            .queue
            .state
            .lock()
            .expect("pty input queue mutex poisoned");
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
    pub async fn recv(&self) -> Option<PtyInput> {
        std::future::poll_fn(|cx| {
            let mut state = self
                .queue
                .state
                .lock()
                .expect("pty input queue mutex poisoned");

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
    pub fn recv_blocking(&self) -> Option<PtyInput> {
        let mut state = self
            .queue
            .state
            .lock()
            .expect("pty input queue mutex poisoned");

        loop {
            if let Some(input) = state.queue.pop_front() {
                return Some(input);
            }

            if state.receiver_closed || state.sender_count == 0 {
                return None;
            }

            state = self
                .queue
                .available
                .wait(state)
                .expect("pty input queue mutex poisoned while waiting");
        }
    }
}

pub fn pty_input_channel() -> (PtyInputSender, PtyInputReceiver) {
    let queue = Arc::new(PtyInputQueue {
        state: Mutex::new(PtyInputQueueState {
            queue: VecDeque::new(),
            sender_count: 1,
            receiver_closed: false,
            waker: None,
        }),
        available: Condvar::new(),
    });

    (
        PtyInputSender {
            queue: Arc::clone(&queue),
        },
        PtyInputReceiver { queue },
    )
}
