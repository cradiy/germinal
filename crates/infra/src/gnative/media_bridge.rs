use std::sync::Arc;

use germinal_gnative_protocol::gnative::media::{
    GNativeAudioPacket, GNativeMediaControlCommand, GNativeVideoPacket,
};

pub trait IGNativeMediaBridge: Send + Sync {
    fn handle_media_control_command(&self, command: GNativeMediaControlCommand);
    fn handle_audio_packet(&self, packet: GNativeAudioPacket);
    fn handle_video_packet(&self, packet: GNativeVideoPacket);
}

#[derive(Debug, Clone, Default)]
pub struct NoopGNativeMediaBridge;

impl IGNativeMediaBridge for NoopGNativeMediaBridge {
    fn handle_media_control_command(&self, _command: GNativeMediaControlCommand) {}

    fn handle_audio_packet(&self, _packet: GNativeAudioPacket) {}

    fn handle_video_packet(&self, _packet: GNativeVideoPacket) {}
}

pub type GNativeMediaBridgeHandle = Arc<dyn IGNativeMediaBridge>;
