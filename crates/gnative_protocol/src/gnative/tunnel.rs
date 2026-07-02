use serde::{Deserialize, Serialize};

use crate::gnative::{
	frame::GNativeFrame,
	input::GNativeInputEvent,
	media::{GNativeAudioPacket, GNativeMediaControlCommand, GNativeVideoPacket},
	session::{GNativeAppHello, GNativeSessionAccepted},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GNativeHostToApp {
	Welcome(GNativeSessionAccepted),
	Mux(GNativeHostMuxFrame),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GNativeAppToHost {
	Hello(GNativeAppHello),
	Mux(GNativeAppMuxFrame),
	Exit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum GNativeStreamPriority {
	Low,
	Normal,
	High,
	Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GNativeStreamKind {
	Control,
	Render,
	Audio,
	Video,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GNativeHostMuxFrame {
	pub mux_seq:  u64,
	pub priority: GNativeStreamPriority,
	pub payload:  GNativeHostPayload,
}

impl GNativeHostMuxFrame {
	pub fn input(mux_seq: u64, input: GNativeInputEvent) -> Self {
		Self {
			mux_seq,
			priority: GNativeStreamPriority::Critical,
			payload: GNativeHostPayload::Input(input),
		}
	}

	pub const fn kind(&self) -> GNativeStreamKind { self.payload.kind() }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GNativeHostPayload {
	Input(GNativeInputEvent),
}

impl GNativeHostPayload {
	pub const fn kind(&self) -> GNativeStreamKind {
		match self {
			Self::Input(_) => GNativeStreamKind::Control,
		}
	}
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GNativeAppMuxFrame {
	pub mux_seq:  u64,
	pub priority: GNativeStreamPriority,
	pub payload:  GNativeAppPayload,
}

impl GNativeAppMuxFrame {
	pub fn new(mux_seq: u64, payload: GNativeAppPayload) -> Self {
		Self { mux_seq, priority: payload.default_priority(), payload }
	}

	pub const fn kind(&self) -> GNativeStreamKind { self.payload.kind() }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GNativeAppPayload {
	Control(GNativeMediaControlCommand),
	Render(GNativeFrame),
	Audio(GNativeAudioPacket),
	Video(GNativeVideoPacket),
}

impl GNativeAppPayload {
	pub const fn kind(&self) -> GNativeStreamKind {
		match self {
			Self::Control(_) => GNativeStreamKind::Control,
			Self::Render(_) => GNativeStreamKind::Render,
			Self::Audio(_) => GNativeStreamKind::Audio,
			Self::Video(_) => GNativeStreamKind::Video,
		}
	}

	pub const fn default_priority(&self) -> GNativeStreamPriority {
		match self {
			Self::Control(_) => GNativeStreamPriority::Critical,
			Self::Render(_) => GNativeStreamPriority::High,
			Self::Audio(_) => GNativeStreamPriority::Normal,
			Self::Video(_) => GNativeStreamPriority::Low,
		}
	}
}

#[cfg(test)]
mod tests {
	use super::{GNativeAppMuxFrame, GNativeAppPayload, GNativeStreamKind, GNativeStreamPriority};
	use crate::gnative::{
		frame::GNativeFrame,
		media::{GNativeAudioPacket, GNativeMediaControlCommand, GNativeVideoPacket},
	};

	#[test]
	fn app_payloads_map_to_expected_stream_priorities() {
		let render_frame = GNativeFrame {
			gshell_id: germinal_domain::gshell::vo::gshell_id::GShellId::new(7),
			seq:       crate::seq::Seq::new(3),
			commands:  Vec::new(),
			cursor:    None,
		};

		assert_eq!(
			GNativeAppMuxFrame::new(1, GNativeAppPayload::Control(GNativeMediaControlCommand::Pause))
				.priority,
			GNativeStreamPriority::Critical
		);
		assert_eq!(
			GNativeAppMuxFrame::new(2, GNativeAppPayload::Render(render_frame.clone())).priority,
			GNativeStreamPriority::High
		);
		assert_eq!(
			GNativeAppMuxFrame::new(
				3,
				GNativeAppPayload::Audio(GNativeAudioPacket {
					stream_id: 1,
					codec:     "aac".to_string(),
					pts_us:    10,
					dts_us:    Some(8),
					payload:   vec![1, 2, 3],
				})
			)
			.priority,
			GNativeStreamPriority::Normal
		);
		assert_eq!(
			GNativeAppMuxFrame::new(
				4,
				GNativeAppPayload::Video(GNativeVideoPacket {
					stream_id: 2,
					codec:     "h264".to_string(),
					pts_us:    11,
					dts_us:    Some(9),
					keyframe:  true,
					payload:   vec![4, 5, 6],
				})
			)
			.priority,
			GNativeStreamPriority::Low
		);
		assert_eq!(
			GNativeAppMuxFrame::new(5, GNativeAppPayload::Render(render_frame)).kind(),
			GNativeStreamKind::Render
		);
	}
}
