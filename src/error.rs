use std::fmt::Debug;
use thiserror::Error;

#[derive(Error)]
pub enum ActorError<T> {
    #[error("Actor error {0}")]
    ActorError(String),
    #[error("Send Error{0}")]
    SendError(#[from] flume::SendError<T>),
    #[error("Recv Error{0}")]
    RecvError(#[from] flume::RecvError),
    #[error("Io Error")]
    IoError(#[from] std::io::Error),
    #[error("Channel closed, no messages left to process")]
    ChannelClosed,
    #[error("System error {0}")]
    SystemError(String),
    #[error("Actor heartbeat timed out {0}")]
    HeartbeatTimeout(crate::ActorId),
    #[error("Unknown")]
    Unknown,
}

impl<T> Debug for ActorError<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ActorError(arg0) => f.debug_tuple("ActorError").field(arg0).finish(),
            Self::SendError(arg0) => f.debug_tuple("SendError").field(arg0).finish(),
            Self::RecvError(arg0) => f.debug_tuple("RecvError").field(arg0).finish(),
            Self::IoError(arg0) => f.debug_tuple("IoError").field(arg0).finish(),
            Self::ChannelClosed => write!(f, "ChannelClosed"),
            Self::SystemError(arg0) => f.debug_tuple("SystemError").field(arg0).finish(),
            Self::HeartbeatTimeout(arg0) => f.debug_tuple("HeartbeatTimeout").field(arg0).finish(),
            Self::Unknown => write!(f, "Unknown"),
        }
    }
}
