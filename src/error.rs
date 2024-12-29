use std::fmt::Debug;

use thiserror::Error;

#[derive(Error)]
pub enum ActorError<T> {
    #[error("Actor error {0}")]
    ActorError(String),

    #[error("Send Error{0}")]
    SendError(#[from] flume::SendError<T>),

    #[error("Io Error")]
    IoError(#[from] std::io::Error),

    #[error("Channel closed, no messages left to process")]
    ChannelClosed,

    #[error("Unknown")]
    Unknown,
}

impl<T> Debug for ActorError<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ActorError(arg0) => f.debug_tuple("ActorError").field(arg0).finish(),
            Self::SendError(arg0) => f.debug_tuple("SendError").field(arg0).finish(),
            Self::IoError(arg0) => f.debug_tuple("IoError").field(arg0).finish(),
            Self::ChannelClosed => write!(f, "ChannelClosed"),
            Self::Unknown => write!(f, "Unknown"),
        }
    }
}
