#[cfg(target_os = "linux")]
use std::os::fd::{BorrowedFd, OwnedFd};
use std::{
    collections::VecDeque,
    path::Path,
    str::FromStr,
    sync::{Arc, Mutex},
    thread,
};

use germinal_gnative_protocol::gnative::media::{
    GNativeAudioPacket, GNativeMediaControlCommand, GNativeVideoPacket,
};
use germinal_ports::event::{
    runtime_event::{RuntimeEvent, WorkspaceRuntimeEvent},
    runtime_event_dispatcher::RuntimeEventDispatchError,
};
#[cfg(target_os = "linux")]
use gst::prelude::*;
#[cfg(target_os = "linux")]
use gstreamer as gst;
#[cfg(target_os = "linux")]
use gstreamer_allocators as gst_allocators;
#[cfg(target_os = "linux")]
use gstreamer_app as gst_app;
#[cfg(target_os = "linux")]
use gstreamer_video as gst_video;
#[cfg(target_os = "linux")]
use nix::unistd::dup;
use thiserror::Error;
#[cfg(not(target_os = "linux"))]
use tracing::debug;
use tracing::{debug, error, warn};

use crate::{
    gnative::media_bridge::IGNativeMediaBridge,
    rendering::pty_surface::video_surface_frame::{
        WgpuVideoSurfaceColorMatrix, WgpuVideoSurfaceColorProfile, WgpuVideoSurfaceColorRange,
        WgpuVideoSurfaceDmaBufPlane, WgpuVideoSurfaceNv12DmaBufFrame,
    },
};

#[derive(Debug, Error)]
pub enum GstVideoPlayerBridgeError {
    #[error("failed to initialize gstreamer: {source}")]
    InitializeGstreamer {
        #[source]
        source: gst::glib::Error,
    },
    #[error("failed to spawn media bridge runtime thread: {source}")]
    SpawnRuntimeThread {
        #[source]
        source: std::io::Error,
    },
    #[error("video path is not a readable file: {path}")]
    UnreadableVideoPath { path: String },
    #[error("no opened video to control")]
    NoOpenedVideoToControl,
    #[error("no opened video to seek")]
    NoOpenedVideoToSeek,
    #[error("failed to build appsink element: {source}")]
    BuildAppsinkElement {
        #[source]
        source: gst::glib::BoolError,
    },
    #[error("failed to convert video path {path} to uri: {source}")]
    VideoPathToUri {
        path: String,
        #[source]
        source: gst::glib::Error,
    },
    #[error("failed to build playbin pipeline for {path}: {source}")]
    BuildPlaybinPipeline {
        path: String,
        #[source]
        source: gst::glib::BoolError,
    },
    #[error("failed to set playbin state to paused for {path}: {source}")]
    PausePipeline {
        path: String,
        #[source]
        source: gst::StateChangeError,
    },
    #[error("failed to change playback state: {source}")]
    ChangePlaybackState {
        #[source]
        source: gst::StateChangeError,
    },
    #[error("failed to seek playback to {position_us}us: {source}")]
    SeekPlayback {
        position_us: u64,
        #[source]
        source: gst::glib::BoolError,
    },
    #[error("failed to create appsink element")]
    CreateAppsinkElement,
    #[error("failed to create DMA_DRM appsink caps: {source}")]
    CreateAppsinkCaps {
        #[source]
        source: gst::glib::BoolError,
    },
    #[error("sample is missing caps")]
    SampleMissingCaps,
    #[error("sample caps are not DMA_DRM")]
    SampleCapsNotDmaDrm,
    #[error("failed to decode DMA_DRM video info from caps: {source}")]
    DecodeVideoInfo {
        #[source]
        source: gst::glib::BoolError,
    },
    #[error("failed to decode DMA_DRM fourcc to video format: {source}")]
    DecodeVideoFormat {
        #[source]
        source: gst::glib::BoolError,
    },
    #[error("unsupported dma-buf video format: {format:?}")]
    UnsupportedVideoFormat { format: gst_video::VideoFormat },
    #[error("sample is missing buffer")]
    SampleMissingBuffer,
    #[error("nv12 sample does not expose two video planes")]
    Nv12PlaneLayoutMissing,
    #[error("video memory is not backed by dmabuf")]
    MemoryNotBackedByDmaBuf,
    #[error(
        "video buffer plane {plane_index} expects memory {memory_index}, but buffer only has \
		 {memory_count} memories"
    )]
    MissingPlaneMemory {
        plane_index: usize,
        memory_index: usize,
        memory_count: usize,
    },
    #[error("invalid negative stride for plane {plane_index}: {stride}")]
    NegativePlaneStride { plane_index: usize, stride: i32 },
    #[error("failed to export DRM dumb memory as dma-buf: {source}")]
    ExportDrmDumbMemory {
        #[source]
        source: gst::glib::BoolError,
    },
    #[error("failed to duplicate dma-buf fd: {source}")]
    DuplicateFd {
        #[source]
        source: nix::errno::Errno,
    },
}

pub struct GstVideoPlayerBridge {
    command_tx: flume::Sender<PlaybackBridgeCommand>,
    pending_frames: Arc<Mutex<VecDeque<PendingVideoSurfaceFrame>>>,
}

type RuntimeEventCallback =
    dyn Fn(RuntimeEvent) -> Result<(), RuntimeEventDispatchError> + Send + Sync;

#[derive(Debug)]
pub struct PendingVideoSurfaceFrame {
    pub surface_id: String,
    pub frame: WgpuVideoSurfaceNv12DmaBufFrame,
}

#[derive(Debug, Clone)]
enum PlaybackBridgeCommand {
    OpenFile { path: String, surface_id: String },
    Play,
    Pause,
    Stop,
    Seek { position_us: u64 },
}

impl GstVideoPlayerBridge {
    pub fn new(dispatch: Arc<RuntimeEventCallback>) -> Result<Self, GstVideoPlayerBridgeError> {
        #[cfg(target_os = "linux")]
        gst::init().map_err(|source| GstVideoPlayerBridgeError::InitializeGstreamer { source })?;

        let pending_frames = Arc::new(Mutex::new(VecDeque::new()));
        let (command_tx, command_rx) = flume::unbounded();
        let runtime_pending_frames = Arc::clone(&pending_frames);

        thread::Builder::new()
            .name("gnative-gst-video-player".to_string())
            .spawn(move || run_playback_runtime(command_rx, runtime_pending_frames, dispatch))
            .map_err(|source| GstVideoPlayerBridgeError::SpawnRuntimeThread { source })?;

        Ok(Self {
            command_tx,
            pending_frames,
        })
    }

    pub fn drain_pending_video_surface_frames(&self) -> Vec<PendingVideoSurfaceFrame> {
        let mut pending_frames = match self.pending_frames.lock() {
            Ok(pending_frames) => pending_frames,
            Err(poisoned) => poisoned.into_inner(),
        };
        pending_frames.drain(..).collect()
    }
}

impl IGNativeMediaBridge for GstVideoPlayerBridge {
    fn handle_media_control_command(&self, command: GNativeMediaControlCommand) {
        let bridge_command = match command {
            GNativeMediaControlCommand::OpenFile { path, surface_id } => {
                PlaybackBridgeCommand::OpenFile { path, surface_id }
            }
            GNativeMediaControlCommand::Play => PlaybackBridgeCommand::Play,
            GNativeMediaControlCommand::Pause => PlaybackBridgeCommand::Pause,
            GNativeMediaControlCommand::Stop => PlaybackBridgeCommand::Stop,
            GNativeMediaControlCommand::Seek { position_us } => {
                PlaybackBridgeCommand::Seek { position_us }
            }
        };

        if let Err(error) = self.command_tx.send(bridge_command) {
            warn!(error = %error, "failed to queue media bridge command");
        }
    }

    fn handle_audio_packet(&self, _packet: GNativeAudioPacket) {}

    fn handle_video_packet(&self, _packet: GNativeVideoPacket) {}
}

#[cfg(not(target_os = "linux"))]
fn run_playback_runtime(
    command_rx: flume::Receiver<PlaybackBridgeCommand>,
    _pending_frames: Arc<Mutex<VecDeque<PendingVideoSurfaceFrame>>>,
    _dispatch: Arc<RuntimeEventCallback>,
) {
    while let Ok(command) = command_rx.recv() {
        debug!(
            ?command,
            "ignored media bridge command on unsupported platform"
        );
    }
}

#[cfg(target_os = "linux")]
fn run_playback_runtime(
    command_rx: flume::Receiver<PlaybackBridgeCommand>,
    pending_frames: Arc<Mutex<VecDeque<PendingVideoSurfaceFrame>>>,
    dispatch: Arc<RuntimeEventCallback>,
) {
    let mut runtime = LinuxPlaybackRuntime::new(pending_frames, dispatch);

    while let Ok(command) = command_rx.recv() {
        if let Err(error) = runtime.handle_command(command) {
            error!(error = %error, "media bridge command failed");
        }
    }

    runtime.stop_current();
}

#[cfg(target_os = "linux")]
struct LinuxPlaybackRuntime {
    pending_frames: Arc<Mutex<VecDeque<PendingVideoSurfaceFrame>>>,
    dispatch: Arc<RuntimeEventCallback>,
    player: Option<OpenedPlayback>,
}

#[cfg(target_os = "linux")]
struct OpenedPlayback {
    pipeline: gst::Element,
}

#[cfg(target_os = "linux")]
impl LinuxPlaybackRuntime {
    fn new(
        pending_frames: Arc<Mutex<VecDeque<PendingVideoSurfaceFrame>>>,
        dispatch: Arc<RuntimeEventCallback>,
    ) -> Self {
        Self {
            pending_frames,
            dispatch,
            player: None,
        }
    }

    fn handle_command(
        &mut self,
        command: PlaybackBridgeCommand,
    ) -> Result<(), GstVideoPlayerBridgeError> {
        match command {
            PlaybackBridgeCommand::OpenFile { path, surface_id } => {
                self.open_file(&path, &surface_id)
            }
            PlaybackBridgeCommand::Play => self.set_state(gst::State::Playing),
            PlaybackBridgeCommand::Pause => self.set_state(gst::State::Paused),
            PlaybackBridgeCommand::Stop => {
                self.stop_current();
                Ok(())
            }
            PlaybackBridgeCommand::Seek { position_us } => self.seek(position_us),
        }
    }

    fn open_file(&mut self, path: &str, surface_id: &str) -> Result<(), GstVideoPlayerBridgeError> {
        let canonical_path = Path::new(path);
        if !canonical_path.is_file() {
            return Err(GstVideoPlayerBridgeError::UnreadableVideoPath {
                path: path.to_string(),
            }
            .into());
        }

        self.stop_current();
        self.clear_pending_frames();

        let uri = gst::glib::filename_to_uri(canonical_path, None).map_err(|source| {
            GstVideoPlayerBridgeError::VideoPathToUri {
                path: path.to_string(),
                source,
            }
        })?;
        let appsink = create_video_appsink(
            surface_id.to_string(),
            Arc::clone(&self.pending_frames),
            Arc::clone(&self.dispatch),
        )?;
        let video_sink = appsink.clone().upcast::<gst::Element>();
        let pipeline = gst::ElementFactory::make("playbin")
            .property("uri", uri.as_str())
            .property("video-sink", &video_sink)
            .build()
            .map_err(|source| GstVideoPlayerBridgeError::BuildPlaybinPipeline {
                path: path.to_string(),
                source,
            })?;

        pipeline.set_state(gst::State::Paused).map_err(|source| {
            GstVideoPlayerBridgeError::PausePipeline {
                path: path.to_string(),
                source,
            }
        })?;
        self.player = Some(OpenedPlayback { pipeline });
        Ok(())
    }

    fn set_state(&self, state: gst::State) -> Result<(), GstVideoPlayerBridgeError> {
        let player = self
            .player
            .as_ref()
            .ok_or(GstVideoPlayerBridgeError::NoOpenedVideoToControl)?;
        player
            .pipeline
            .set_state(state)
            .map_err(|source| GstVideoPlayerBridgeError::ChangePlaybackState { source })?;
        Ok(())
    }

    fn seek(&self, position_us: u64) -> Result<(), GstVideoPlayerBridgeError> {
        let player = self
            .player
            .as_ref()
            .ok_or(GstVideoPlayerBridgeError::NoOpenedVideoToSeek)?;
        player
            .pipeline
            .seek_simple(
                gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT,
                gst::ClockTime::from_useconds(position_us),
            )
            .map_err(|source| GstVideoPlayerBridgeError::SeekPlayback {
                position_us,
                source,
            })?;
        Ok(())
    }

    fn stop_current(&mut self) {
        self.clear_pending_frames();
        if let Some(player) = self.player.take() {
            let _ = player.pipeline.set_state(gst::State::Null);
        }
    }

    fn clear_pending_frames(&self) {
        let mut pending_frames = match self.pending_frames.lock() {
            Ok(pending_frames) => pending_frames,
            Err(poisoned) => poisoned.into_inner(),
        };
        pending_frames.clear();
    }
}

#[cfg(target_os = "linux")]
fn create_video_appsink(
    surface_id: String,
    pending_frames: Arc<Mutex<VecDeque<PendingVideoSurfaceFrame>>>,
    dispatch: Arc<RuntimeEventCallback>,
) -> Result<gst_app::AppSink, GstVideoPlayerBridgeError> {
    let appsink = gst::ElementFactory::make("appsink")
        .property("name", "gnative-video-appsink")
        .build()
        .map_err(|source| GstVideoPlayerBridgeError::BuildAppsinkElement { source })?
        .downcast::<gst_app::AppSink>()
        .map_err(|_| GstVideoPlayerBridgeError::CreateAppsinkElement)?;

    let caps = gst::Caps::from_str("video/x-raw(memory:DMABuf),format=DMA_DRM")
        .map_err(|source| GstVideoPlayerBridgeError::CreateAppsinkCaps { source })?;
    appsink.set_caps(Some(&caps));
    appsink.set_max_buffers(2);
    appsink.set_wait_on_eos(false);
    appsink.set_callbacks(
        gst_app::AppSinkCallbacks::builder()
            .propose_allocation(|_, query| {
                query.add_allocation_meta::<gst_video::VideoMeta>(None);
                true
            })
            .new_preroll({
                let surface_id = surface_id.clone();
                let pending_frames = Arc::clone(&pending_frames);
                let dispatch = Arc::clone(&dispatch);
                move |appsink| {
                    handle_new_video_preroll(appsink, &surface_id, &pending_frames, &dispatch)
                }
            })
            .new_sample(move |appsink| {
                handle_new_video_sample(appsink, &surface_id, &pending_frames, &dispatch)
            })
            .build(),
    );
    Ok(appsink)
}

#[cfg(target_os = "linux")]
fn handle_new_video_preroll(
    appsink: &gst_app::AppSink,
    surface_id: &str,
    pending_frames: &Arc<Mutex<VecDeque<PendingVideoSurfaceFrame>>>,
    dispatch: &Arc<RuntimeEventCallback>,
) -> Result<gst::FlowSuccess, gst::FlowError> {
    let sample = appsink.pull_preroll().map_err(|_| gst::FlowError::Error)?;
    queue_video_sample(sample, surface_id, pending_frames, dispatch)
}

#[cfg(target_os = "linux")]
fn handle_new_video_sample(
    appsink: &gst_app::AppSink,
    surface_id: &str,
    pending_frames: &Arc<Mutex<VecDeque<PendingVideoSurfaceFrame>>>,
    dispatch: &Arc<RuntimeEventCallback>,
) -> Result<gst::FlowSuccess, gst::FlowError> {
    let sample = appsink.pull_sample().map_err(|_| gst::FlowError::Error)?;
    queue_video_sample(sample, surface_id, pending_frames, dispatch)
}

#[cfg(target_os = "linux")]
fn queue_video_sample(
    sample: gst::Sample,
    surface_id: &str,
    pending_frames: &Arc<Mutex<VecDeque<PendingVideoSurfaceFrame>>>,
    dispatch: &Arc<RuntimeEventCallback>,
) -> Result<gst::FlowSuccess, gst::FlowError> {
    let frame = video_surface_frame_from_sample(&sample).map_err(|error| {
        debug!(error = %error, "failed to import gstreamer video sample");
        gst::FlowError::NotNegotiated
    })?;

    {
        let mut queue = match pending_frames.lock() {
            Ok(queue) => queue,
            Err(poisoned) => poisoned.into_inner(),
        };
        queue.push_back(PendingVideoSurfaceFrame {
            surface_id: surface_id.to_string(),
            frame,
        });
        while queue.len() > 2 {
            queue.pop_front();
        }
    }

    if let Err(error) = dispatch(RuntimeEvent::Workspace(
        WorkspaceRuntimeEvent::RedrawRequested,
    )) {
        warn!(
            surface_id,
            error = %error,
            "failed to dispatch redraw request for imported video frame"
        );
    }
    Ok(gst::FlowSuccess::Ok)
}

#[cfg(target_os = "linux")]
fn video_surface_frame_from_sample(
    sample: &gst::Sample,
) -> Result<WgpuVideoSurfaceNv12DmaBufFrame, GstVideoPlayerBridgeError> {
    let caps = sample
        .caps()
        .ok_or(GstVideoPlayerBridgeError::SampleMissingCaps)?;
    if !gst_video::is_dma_drm_caps(caps) {
        return Err(GstVideoPlayerBridgeError::SampleCapsNotDmaDrm.into());
    }

    let info = gst_video::VideoInfoDmaDrm::from_caps(caps)
        .map_err(|source| GstVideoPlayerBridgeError::DecodeVideoInfo { source })?;
    let format = gst_video::dma_drm_fourcc_to_format(info.fourcc())
        .map_err(|source| GstVideoPlayerBridgeError::DecodeVideoFormat { source })?;
    if format != gst_video::VideoFormat::Nv12 {
        return Err(GstVideoPlayerBridgeError::UnsupportedVideoFormat { format }.into());
    }

    let buffer = sample
        .buffer()
        .ok_or(GstVideoPlayerBridgeError::SampleMissingBuffer)?;
    let (offsets, strides) = plane_layout(buffer, &info);
    if offsets.len() < 2 || strides.len() < 2 {
        return Err(GstVideoPlayerBridgeError::Nv12PlaneLayoutMissing.into());
    }
    let color_profile = color_profile_from_video_info(&info);

    let y_plane = dma_buf_plane_from_memory(
        buffer,
        0,
        if buffer.n_memory() == 1 { 0 } else { 0 },
        offsets[0],
        strides[0],
        info.modifier(),
    )?;
    let uv_plane = dma_buf_plane_from_memory(
        buffer,
        1,
        if buffer.n_memory() == 1 { 0 } else { 1 },
        offsets[1],
        strides[1],
        info.modifier(),
    )?;

    Ok(WgpuVideoSurfaceNv12DmaBufFrame {
        width_px: info.width(),
        height_px: info.height(),
        color_profile,
        y_plane,
        uv_plane,
    })
}

#[cfg(target_os = "linux")]
fn color_profile_from_video_info(
    info: &gst_video::VideoInfoDmaDrm,
) -> WgpuVideoSurfaceColorProfile {
    let colorimetry = info.colorimetry();
    let range = match colorimetry.range() {
        gst_video::VideoColorRange::Range0_255 => WgpuVideoSurfaceColorRange::Full,
        gst_video::VideoColorRange::Range16_235 | gst_video::VideoColorRange::Unknown => {
            WgpuVideoSurfaceColorRange::Limited
        }
        _ => WgpuVideoSurfaceColorRange::Limited,
    };

    let matrix = match colorimetry.matrix() {
        gst_video::VideoColorMatrix::Bt601 => WgpuVideoSurfaceColorMatrix::Bt601,
        gst_video::VideoColorMatrix::Bt709 => WgpuVideoSurfaceColorMatrix::Bt709,
        gst_video::VideoColorMatrix::Unknown | gst_video::VideoColorMatrix::__Unknown(_) => {
            default_matrix_for_resolution(info.width(), info.height())
        }
        _ => WgpuVideoSurfaceColorMatrix::Bt709,
    };

    WgpuVideoSurfaceColorProfile { range, matrix }
}

#[cfg(target_os = "linux")]
fn default_matrix_for_resolution(width_px: u32, height_px: u32) -> WgpuVideoSurfaceColorMatrix {
    if width_px >= 1280 || height_px > 576 {
        WgpuVideoSurfaceColorMatrix::Bt709
    } else {
        WgpuVideoSurfaceColorMatrix::Bt601
    }
}

#[cfg(target_os = "linux")]
fn plane_layout(
    buffer: &gst::BufferRef,
    info: &gst_video::VideoInfoDmaDrm,
) -> (Vec<usize>, Vec<i32>) {
    if let Some(meta) = buffer.meta::<gst_video::VideoMeta>() {
        return (meta.offset().to_vec(), meta.stride().to_vec());
    }

    (info.offset().to_vec(), info.stride().to_vec())
}

#[cfg(target_os = "linux")]
fn dma_buf_plane_from_memory(
    buffer: &gst::BufferRef,
    plane_index: usize,
    memory_index: usize,
    offset: usize,
    stride: i32,
    modifier: u64,
) -> Result<WgpuVideoSurfaceDmaBufPlane, GstVideoPlayerBridgeError> {
    if memory_index >= buffer.n_memory() {
        return Err(GstVideoPlayerBridgeError::MissingPlaneMemory {
            plane_index,
            memory_index,
            memory_count: buffer.n_memory(),
        }
        .into());
    }

    let memory = buffer.peek_memory(memory_index);
    let fd = duplicate_dmabuf_fd(memory)?;
    let stride =
        u32::try_from(stride).map_err(|_| GstVideoPlayerBridgeError::NegativePlaneStride {
            plane_index,
            stride,
        })?;

    Ok(WgpuVideoSurfaceDmaBufPlane {
        fd: Arc::new(fd),
        offset: if buffer.n_memory() == 1 {
            offset as u64
        } else {
            0
        },
        stride,
        modifier,
    })
}

#[cfg(target_os = "linux")]
fn duplicate_dmabuf_fd(memory: &gst::MemoryRef) -> Result<OwnedFd, GstVideoPlayerBridgeError> {
    if let Some(dmabuf) = memory.downcast_memory_ref::<gst_allocators::DmaBufMemory>() {
        return duplicate_raw_fd(dmabuf.fd());
    }

    if let Some(drm_dumb) = memory.downcast_memory_ref::<gst_allocators::DRMDumbMemory>() {
        let exported = drm_dumb
            .export_dmabuf()
            .map_err(|source| GstVideoPlayerBridgeError::ExportDrmDumbMemory { source })?;
        return duplicate_raw_fd(exported.fd());
    }

    Err(GstVideoPlayerBridgeError::MemoryNotBackedByDmaBuf.into())
}

#[cfg(target_os = "linux")]
fn duplicate_raw_fd(fd: i32) -> Result<OwnedFd, GstVideoPlayerBridgeError> {
    let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
    dup(borrowed).map_err(|source| GstVideoPlayerBridgeError::DuplicateFd { source })
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::{
        env,
        time::{Duration, Instant},
    };

    use super::*;

    #[test]
    #[ignore = "manual smoke test; set GERMINAL_VIDEO_SMOKE_PATH to a local playable file"]
    fn bridge_emits_pending_dmabuf_frames_for_local_video() {
        let path = env::var("GERMINAL_VIDEO_SMOKE_PATH")
            .expect("GERMINAL_VIDEO_SMOKE_PATH must point to a local video file");
        let bridge =
            GstVideoPlayerBridge::new(Arc::new(|_| Ok(()))).expect("bridge should initialize");

        bridge.handle_media_control_command(GNativeMediaControlCommand::OpenFile {
            path,
            surface_id: "video-player-surface".to_string(),
        });
        bridge.handle_media_control_command(GNativeMediaControlCommand::Play);

        let started_at = Instant::now();
        while started_at.elapsed() < Duration::from_secs(5) {
            if !bridge.drain_pending_video_surface_frames().is_empty() {
                return;
            }
            thread::sleep(Duration::from_millis(50));
        }

        panic!("bridge did not emit any pending video surface frame within timeout");
    }
}
