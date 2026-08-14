use std::{
    process::{Command, Stdio},
    sync::mpsc::{self, SyncSender, TrySendError},
    thread,
};

use tracing::warn;

use super::config::BellCommand;

pub struct AudibleBell {
    trigger: Option<SyncSender<()>>,
}

impl AudibleBell {
    pub fn new(command: Option<BellCommand>) -> Self {
        let Some(command) = command.filter(|command| !command.program.trim().is_empty()) else {
            return Self { trigger: None };
        };

        let (trigger, receiver) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("germinal-audible-bell".to_string())
            .spawn(move || {
                while receiver.recv().is_ok() {
                    let result = Command::new(&command.program)
                        .args(&command.args)
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status();

                    if let Err(error) = result {
                        warn!(
                            program = %command.program,
                            %error,
                            "failed to execute audible bell command"
                        );
                    }
                }
            });

        match worker {
            Ok(_) => Self {
                trigger: Some(trigger),
            },
            Err(error) => {
                warn!(%error, "failed to start audible bell worker");
                Self { trigger: None }
            }
        }
    }

    pub fn ring(&self) {
        let Some(trigger) = &self.trigger else {
            return;
        };

        match trigger.try_send(()) {
            Ok(()) | Err(TrySendError::Full(())) => {}
            Err(TrySendError::Disconnected(())) => {
                warn!("audible bell worker is no longer available");
            }
        }
    }
}
