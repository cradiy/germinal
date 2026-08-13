use std::{io, num::ParseIntError};

use thiserror::Error;

pub type GNativeSdkResult<T> = Result<T, GNativeSdkError>;

#[derive(Debug, Error)]
pub enum GNativeSdkError {
	#[error("missing environment variable {name}")]
	MissingEnv { name: &'static str },
	#[error("invalid numeric environment variable {name}: {source}")]
	InvalidEnvNumber {
		name:   &'static str,
		#[source]
		source: ParseIntError,
	},
	#[error("I/O error: {0}")]
	Io(#[from] io::Error),
	#[error("failed to encode protocol message: {0}")]
	EncodeMessage(#[source] serde_json::Error),
	#[error("failed to decode protocol message: {0}")]
	DecodeMessage(#[source] serde_json::Error),
	#[error("host closed before sending welcome")]
	HostClosedBeforeWelcome,
	#[error("expected welcome after gnative hello")]
	ExpectedWelcomeAfterHello,
	#[error("unexpected duplicate welcome message")]
	UnexpectedDuplicateWelcome,
	#[error("frame gshell_id does not match accepted session")]
	FrameGshellMismatch,
	#[error("gnative outbound queue is closed")]
	OutboundQueueClosed,
	#[error("gnative host message exceeds the {max_bytes}-byte limit")]
	MessageTooLarge { max_bytes: usize },
}
