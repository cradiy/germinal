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
#[cfg(windows)]
const WINDOWS_APP_ID: &str = "io.github.cradiy.Germinal";

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
                #[cfg(windows)]
                let mut identity_ready = false;
                while let Ok(request) = receiver.recv() {
                    #[cfg(windows)]
                    if !identity_ready {
                        if let Err(error) = register_windows_notification_identity() {
                            warn!(%error, "failed to register Germinal notification identity");
                            continue;
                        }
                        identity_ready = true;
                    }
                    let focus_on_activation = request.notification.focus_on_activation;
                    let mut notification = platform_notification(request.notification);
                    if focus_on_activation {
                        notification.action("default", "Open");
                    }
                    match notification.show() {
                        Ok(handle) if focus_on_activation => {
                            let on_activation = Arc::clone(&on_activation);
                            spawn_activation_waiter(
                                move || {
                                    #[cfg(windows)]
                                    if let Err(error) = handle.wait_for_response(
                                        |response: &notify_rust::NotificationResponse| {
                                            if !matches!(response, notify_rust::NotificationResponse::Closed(_)) {
                                                on_activation(request.gshell_id);
                                            }
                                        },
                                    ) {
                                        warn!(%error, "failed to wait for terminal notification activation");
                                    }
                                    #[cfg(not(windows))]
                                    handle.wait_for_action(|action| {
                                        if action != "__closed" {
                                            on_activation(request.gshell_id);
                                        }
                                    });
                                },
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

fn spawn_activation_waiter<F>(wait_for_activation: F, waiter_count: Arc<AtomicUsize>)
where
    F: FnOnce() + Send + 'static,
{
    if waiter_count
        .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
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
            wait_for_activation();
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
    #[cfg(windows)]
    platform_notification.app_id(WINDOWS_APP_ID);
    if let Some(body) = body {
        platform_notification.body(&body);
    }
    platform_notification
}

#[cfg(windows)]
fn register_windows_notification_identity() -> std::io::Result<()> {
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_SZ, RegCloseKey, RegCreateKeyW, RegSetValueExW,
    };
    // Unpackaged WinRT toast identity, following tauri-winrt-notification's
    // unpackaged_app example. Only our own per-user application key is written.
    let path: Vec<u16> = format!("Software\\Classes\\AppUserModelId\\{WINDOWS_APP_ID}\0")
        .encode_utf16()
        .collect();
    let name: Vec<u16> = "DisplayName\0".encode_utf16().collect();
    let value: Vec<u16> = format!("{APPLICATION_NAME}\0").encode_utf16().collect();
    let mut key = std::ptr::null_mut();
    let result = unsafe { RegCreateKeyW(HKEY_CURRENT_USER, path.as_ptr(), &mut key) };
    if result != 0 {
        return Err(std::io::Error::from_raw_os_error(result as i32));
    }
    let result = unsafe {
        let result = RegSetValueExW(
            key,
            name.as_ptr(),
            0,
            REG_SZ,
            value.as_ptr().cast(),
            (value.len() * 2) as u32,
        );
        RegCloseKey(key);
        result
    };
    if result != 0 {
        return Err(std::io::Error::from_raw_os_error(result as i32));
    }
    Ok(())
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
