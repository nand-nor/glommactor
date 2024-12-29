mod actor;
mod error;
pub mod handle;
mod supervisor;

pub use actor::{Actor, ActorState, Event, SupervisedActor};
pub use error::ActorError;
pub use supervisor::{Supervisor, SupervisorHandle, SupervisorMessage};

pub type ActorId = u16;
