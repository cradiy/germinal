use std::{
    collections::HashMap,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{RecvTimeoutError, TryRecvError},
    },
    time::Duration,
};

use thiserror::Error;

use super::{
    render_target_id::RenderTargetId,
    surface_snapshot::{RenderSurfaceSnapshot, merge_surface_dirty_rows},
};

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
#[error("surface snapshot mailbox is closed")]
pub struct SurfaceSnapshotMailboxClosed;

#[derive(Clone)]
pub struct SurfaceSnapshotSender {
    inner: Arc<SurfaceSnapshotMailboxInner>,
}

pub struct SurfaceSnapshotReceiver {
    inner: Arc<SurfaceSnapshotMailboxInner>,
}

struct SurfaceSnapshotMailboxInner {
    snapshots: Mutex<HashMap<RenderTargetId, RenderSurfaceSnapshot>>,
    snapshot_ready: Condvar,
    receiver_alive: AtomicBool,
}

pub fn surface_snapshot_mailbox() -> (SurfaceSnapshotSender, SurfaceSnapshotReceiver) {
    let inner = Arc::new(SurfaceSnapshotMailboxInner {
        snapshots: Mutex::new(HashMap::new()),
        snapshot_ready: Condvar::new(),
        receiver_alive: AtomicBool::new(true),
    });
    (
        SurfaceSnapshotSender {
            inner: Arc::clone(&inner),
        },
        SurfaceSnapshotReceiver { inner },
    )
}

impl SurfaceSnapshotSender {
    pub fn send(
        &self,
        mut snapshot: RenderSurfaceSnapshot,
    ) -> Result<(), SurfaceSnapshotMailboxClosed> {
        if !self.inner.receiver_alive.load(Ordering::Acquire) {
            return Err(SurfaceSnapshotMailboxClosed);
        }

        let mut snapshots = match self.inner.snapshots.lock() {
            Ok(snapshots) => snapshots,
            Err(poisoned) => poisoned.into_inner(),
        };
        if !self.inner.receiver_alive.load(Ordering::Acquire) {
            return Err(SurfaceSnapshotMailboxClosed);
        }
        if let Some(current) = snapshots.get(&snapshot.target_id) {
            if snapshot.latest_seq < current.latest_seq {
                return Ok(());
            }
            merge_surface_dirty_rows(&mut snapshot.dirty_rows, &current.dirty_rows);
        }
        snapshots.insert(snapshot.target_id, snapshot);
        drop(snapshots);
        self.inner.snapshot_ready.notify_one();
        Ok(())
    }
}

impl SurfaceSnapshotReceiver {
    pub fn try_recv(&self) -> Result<RenderSurfaceSnapshot, TryRecvError> {
        let mut snapshots = match self.inner.snapshots.lock() {
            Ok(snapshots) => snapshots,
            Err(poisoned) => poisoned.into_inner(),
        };
        take_one(&mut snapshots).ok_or(TryRecvError::Empty)
    }

    pub fn recv_timeout(
        &self,
        timeout: Duration,
    ) -> Result<RenderSurfaceSnapshot, RecvTimeoutError> {
        let snapshots = match self.inner.snapshots.lock() {
            Ok(snapshots) => snapshots,
            Err(poisoned) => poisoned.into_inner(),
        };
        let mut snapshots =
            match self
                .inner
                .snapshot_ready
                .wait_timeout_while(snapshots, timeout, |snapshots| snapshots.is_empty())
            {
                Ok((snapshots, _)) => snapshots,
                Err(poisoned) => poisoned.into_inner().0,
            };
        take_one(&mut snapshots).ok_or(RecvTimeoutError::Timeout)
    }

    pub fn try_iter(&self) -> impl Iterator<Item = RenderSurfaceSnapshot> + '_ {
        std::iter::from_fn(|| self.try_recv().ok())
    }
}

impl Drop for SurfaceSnapshotReceiver {
    fn drop(&mut self) {
        self.inner.receiver_alive.store(false, Ordering::Release);
        self.inner.snapshot_ready.notify_all();
    }
}

fn take_one(
    snapshots: &mut HashMap<RenderTargetId, RenderSurfaceSnapshot>,
) -> Option<RenderSurfaceSnapshot> {
    let target_id = snapshots.keys().next().copied()?;
    snapshots.remove(&target_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{rendering::frame_plan_builder::RgbColorDto, seq::Seq};

    fn snapshot(target_id: u64, seq: u64, dirty_rows: Vec<u32>) -> RenderSurfaceSnapshot {
        RenderSurfaceSnapshot {
            target_id: RenderTargetId::new(target_id),
            latest_seq: Seq::new(seq),
            default_background: RgbColorDto::new(0, 0, 0),
            rows: Vec::new(),
            image_surfaces: Vec::new(),
            dirty_rows,
            cursor: None,
            ime_preedit: None,
        }
    }

    #[test]
    fn keeps_only_the_latest_snapshot_per_target_and_merges_damage() {
        let (sender, receiver) = surface_snapshot_mailbox();
        sender.send(snapshot(1, 1, vec![1])).unwrap();
        sender.send(snapshot(1, 2, vec![2])).unwrap();
        sender.send(snapshot(2, 1, vec![4])).unwrap();

        let mut received = [receiver.try_recv().unwrap(), receiver.try_recv().unwrap()];
        received.sort_by_key(|snapshot| snapshot.target_id.value());

        assert_eq!(received[0].latest_seq, Seq::new(2));
        assert_eq!(received[0].dirty_rows, vec![1, 2]);
        assert_eq!(received[1].target_id, RenderTargetId::new(2));
        assert_eq!(receiver.try_recv(), Err(TryRecvError::Empty));
    }

    #[test]
    fn rejects_stale_snapshots_without_replacing_the_latest() {
        let (sender, receiver) = surface_snapshot_mailbox();
        sender.send(snapshot(1, 2, vec![2])).unwrap();
        sender.send(snapshot(1, 1, vec![1])).unwrap();

        let received = receiver.try_recv().unwrap();
        assert_eq!(received.latest_seq, Seq::new(2));
        assert_eq!(received.dirty_rows, vec![2]);
    }

    #[test]
    fn send_fails_after_the_receiver_is_dropped() {
        let (sender, receiver) = surface_snapshot_mailbox();
        drop(receiver);

        assert_eq!(
            sender.send(snapshot(1, 1, vec![])),
            Err(SurfaceSnapshotMailboxClosed)
        );
    }
}
