#[async_trait::async_trait]
pub trait Actor<T: Event + Send>: Sized + Unpin + 'static {
    type Rx;
    type Error;
    type Result;
    async fn run(self) -> Self::Result;
}

#[derive(Clone, Eq, PartialEq)]
pub enum ActorState {
    Stopped,
    Started,
    Running,
    Stopping,
}

// marker trait
pub trait Event {}
