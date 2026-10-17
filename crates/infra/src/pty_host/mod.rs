pub mod alacritty_terminal_store;
mod compatibility_protocol;
mod kitty_graphics;
mod kitty_terminal_lifecycle;
mod notification_protocol;
#[cfg(all(test, windows))]
pub(crate) use notification_protocol::{
    NotificationProtocolEvent, TerminalNotificationProtocolDecoder,
};
pub mod worker;
