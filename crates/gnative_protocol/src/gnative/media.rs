use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GNativeMediaControlCommand {
    OpenFile { path: String, surface_id: String },
    Play,
    Pause,
    Stop,
    Seek { position_us: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GNativeAudioPacket {
    pub stream_id: u32,
    pub codec: String,
    pub pts_us: u64,
    pub dts_us: Option<u64>,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GNativeVideoPacket {
    pub stream_id: u32,
    pub codec: String,
    pub pts_us: u64,
    pub dts_us: Option<u64>,
    pub keyframe: bool,
    pub payload: Vec<u8>,
}
