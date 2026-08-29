use serde::{Deserialize, Serialize};

use crate::gnative::{
    frame::GNativeFrame,
    input::GNativeInputEvent,
    session::{GNativeAppHello, GNativeSessionAccepted},
};

pub const GNATIVE_MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GNativeHostToApp {
    Welcome(GNativeSessionAccepted),
    Input(GNativeInputEvent),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GNativeAppToHost {
    Hello(GNativeAppHello),
    Frame(GNativeFrame),
    Exit,
}
