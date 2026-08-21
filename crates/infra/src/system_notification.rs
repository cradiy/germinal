use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc::{self, SyncSender, TrySendError},
    },
    thread,
};

use germinal_domain::gshell::vo::gshell_id::GShellId;
use germinal_ports::pty_host::terminal_notification::TerminalNotification;
use notify_rust::Notification;
use tracing::{debug, warn};

const NOTIFICATION_QUEUE_CAPACITY: usize = 64;
const MAX_ACTIVATION_WAITERS: usize = 64;
const APPLICATION_NAME: &str = "Germinal";

struct SystemNotificationRequest {
    gshell_id: GShellId,
    notification: TerminalNotification,
}

pub struct SystemNotifier {
    sender: Option<SyncSender<SystemNotificationRequest>>,
}

impl SystemNotifier {
    pub fn new<F>(on_activation: F) -> Self
    where
        F: Fn(GShellId) + Send + Sync + 'static,
    {
        let (sender, receiver) =
            mpsc::sync_channel::<SystemNotificationRequest>(NOTIFICATION_QUEUE_CAPACITY);
        let on_activation = Arc::new(on_activation);
        let activation_waiters = Arc::new(AtomicUsize::new(0));
        let worker = thread::Builder::new()
            .name("germinal-system-notifications".to_owned())
            .spawn(move || {
                while let Ok(request) = receiver.recv() {
                    let focus_on_activation = request.notification.focus_on_activation;
                    let mut notification = platform_notification(request.notification);
                    if focus_on_activation {
                        notification.action("default", "Open");
                    }
                    match notification.show() {
                        Ok(handle) if focus_on_activation => {
                            spawn_activation_waiter(
                                handle,
                                request.gshell_id,
                                Arc::clone(&on_activation),
                                Arc::clone(&activation_waiters),
                            );
                        }
                        Ok(_) => {}
                        Err(error) => {
                            warn!(%error, "failed to show terminal system notification");
                        }
                    }
                }
            });

        match worker {
            Ok(_) => Self {
                sender: Some(sender),
            },
            Err(error) => {
                warn!(%error, "failed to start system notification worker");
                Self { sender: None }
            }
        }
    }

    pub fn show(&self, gshell_id: GShellId, notification: TerminalNotification) {
        let Some(sender) = &self.sender else {
            return;
        };

        match sender.try_send(SystemNotificationRequest {
            gshell_id,
            notification,
        }) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                debug!("dropping terminal notification because the queue is full");
            }
            Err(TrySendError::Disconnected(_)) => {
                warn!("system notification worker is no longer available");
            }
        }
    }
}

fn spawn_activation_waiter<F>(
    handle: notify_rust::NotificationHandle,
    gshell_id: GShellId,
    on_activation: Arc<F>,
    waiter_count: Arc<AtomicUsize>,
) where
    F: Fn(GShellId) + Send + Sync + 'static + ?Sized,
{
    if waiter_count
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
            (count < MAX_ACTIVATION_WAITERS).then_some(count + 1)
        })
        .is_err()
    {
        debug!(
            "not tracking terminal notification activation because the waiter limit was reached"
        );
        return;
    }

    let worker_waiter_count = Arc::clone(&waiter_count);
    let worker = thread::Builder::new()
        .name("germinal-notification-activation".to_owned())
        .spawn(move || {
            handle.wait_for_action(|action| {
                if action != "__closed" {
                    on_activation(gshell_id);
                }
            });
            worker_waiter_count.fetch_sub(1, Ordering::AcqRel);
        });
    if let Err(error) = worker {
        waiter_count.fetch_sub(1, Ordering::AcqRel);
        warn!(%error, "failed to start system notification activation waiter");
    }
}

fn platform_notification(notification: TerminalNotification) -> Notification {
    let (summary, body) = notification_content(notification);
    let mut platform_notification = Notification::new();
    platform_notification
        .appname(APPLICATION_NAME)
        .summary(&summary);
    if let Some(body) = body {
        platform_notification.body(&body);
    }
    platform_notification
}

fn notification_content(notification: TerminalNotification) -> (String, Option<String>) {
    match (notification.title, notification.body) {
        (Some(title), body) => (title, body),
        (None, Some(body)) => (APPLICATION_NAME.to_owned(), Some(body)),
        (None, None) => (APPLICATION_NAME.to_owned(), None),
    }
}

#[cfg(test)]
mod tests {
    use germinal_ports::pty_host::terminal_notification::TerminalNotificationOccasion;

    use super::*;

    #[test]
    fn body_only_notifications_use_germinal_as_the_summary() {
        assert_eq!(
            notification_content(TerminalNotification::new(
                None,
                Some("build finished".to_owned()),
                TerminalNotificationOccasion::Always,
            )),
            (
                APPLICATION_NAME.to_owned(),
                Some("build finished".to_owned())
            )
        );
    }

    #[test]
    fn explicit_titles_are_used_as_the_summary() {
        assert_eq!(
            notification_content(TerminalNotification::new(
                Some("Cargo".to_owned()),
                Some("tests passed".to_owned()),
                TerminalNotificationOccasion::Always,
            )),
            ("Cargo".to_owned(), Some("tests passed".to_owned()))
        );
    }
}
