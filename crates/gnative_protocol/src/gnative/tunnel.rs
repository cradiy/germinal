use serde::{Deserialize, Serialize};

use crate::gnative::{
	frame::GNativeFrame,
	input::GNativeInputEvent,
	session::{GNativeAppHello, GNativeSessionAccepted},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
