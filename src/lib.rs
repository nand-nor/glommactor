//! A very simple actor framework, built for the glommio runtime

mod actor;
mod error;
pub mod handle;
mod supervisor;

pub use actor::{Actor, ActorState, Event, SupervisedActor};
pub use error::ActorError;
pub use supervisor::{Supervision, Supervisor, SupervisorHandle, SupervisorMessage};

pub type ActorId = u16;
