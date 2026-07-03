use std::sync::mpsc;

use germinal_domain::gshell::vo::gshell_id::GShellId;
use germinal_gnative_protocol::gnative::{
	input::GNativeInputEvent,
	session::{GNativeSessionAccepted, GNativeSessionDescriptor},
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GNativeTunnelError {
	#[error("failed to create gnative tunnel async runtime: {source}")]
	CreateRuntime {
		#[source]
		source: std::io::Error,
	},
	#[error("failed to spawn gnative tunnel runtime thread: {source}")]
	SpawnRuntimeThread {
		#[source]
		source: std::io::Error,
	},
	#[error("gnative tunnel runtime thread exited before reporting initialization: {0}")]
	RuntimeBootstrapChannelClosed(#[source] mpsc::RecvError),
	#[error("gnative tunnel command channel is closed")]
	CommandChannelClosed,
	#[error("gnative tunnel response channel is closed: {0}")]
	ResponseChannelClosed(#[source] mpsc::RecvError),
	#[error("gnative tunnel is not configured with a dispatcher")]
	DispatcherNotConfigured,
	#[error("gnative tunnel is not configured with a snapshot sender")]
	SnapshotSenderNotConfigured,
	#[error("no gnative tunnel slot for gshell {gshell_id}")]
	MissingTunnelSlot { gshell_id: u64 },
	#[error("no active gnative tunnel session for gshell {gshell_id}")]
	InactiveSession { gshell_id: u64 },
	#[error("gnative app closed before hello")]
	AppClosedBeforeHello,
	#[error("expected gnative hello during handshake")]
	UnexpectedHandshakeMessage,
	#[error("gnative hello token mismatch")]
	TokenMismatch,
	#[error("gnative protocol version mismatch: expected {expected}, got {actual}")]
	ProtocolVersionMismatch { expected: u32, actual: u32 },
	#[error("failed to bind gnative tunnel listener: {source}")]
	BindListener {
		#[source]
		source: std::io::Error,
	},
	#[error("failed to resolve gnative tunnel listener address: {source}")]
	ResolveListenerAddress {
		#[source]
		source: std::io::Error,
	},
	#[error("failed to accept gnative app connection for gshell {gshell_id}: {source}")]
	AcceptConnection {
		gshell_id: u64,
		#[source]
		source:    std::io::Error,
	},
	#[error("failed to write gnative host message: {source}")]
	WriteMessage {
		#[source]
		source: std::io::Error,
	},
	#[error("failed to flush gnative host message: {source}")]
	FlushMessage {
		#[source]
		source: std::io::Error,
	},
	#[error("failed to read gnative app message: {source}")]
	ReadMessage {
		#[source]
		source: std::io::Error,
	},
	#[error("failed to shut down gnative session for gshell {gshell_id}: {source}")]
	ShutdownSession {
		gshell_id: u64,
		#[source]
		source:    std::io::Error,
	},
	#[error("failed to encode gnative tunnel message: {0}")]
	EncodeMessage(#[source] serde_json::Error),
	#[error("failed to decode gnative tunnel message: {0}")]
	DecodeMessage(#[source] serde_json::Error),
	#[error("gnative app closed mid-message")]
	AppClosedMidMessage,
}

pub trait IGNativeTunnel {
	fn ensure_session_descriptor(
		&self,
		gshell_id: GShellId,
		protocol_version: u32,
	) -> Result<GNativeSessionDescriptor, GNativeTunnelError>;
	fn accept_session(
		&self,
		gshell_id: GShellId,
	) -> Result<GNativeSessionAccepted, GNativeTunnelError>;
	fn send_input(
		&self,
		gshell_id: GShellId,
		input: GNativeInputEvent,
	) -> Result<(), GNativeTunnelError>;
	fn close_session(&self, gshell_id: GShellId) -> Result<(), GNativeTunnelError>;
}

pub trait IGNativeTunnelProvider {
	type GNativeTunnel: IGNativeTunnel;

	fn gnative_tunnel(&self) -> &Self::GNativeTunnel;
}
