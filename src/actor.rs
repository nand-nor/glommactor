#[async_trait::async_trait]
pub trait Actor<T: Event + Send> {
    type Rx;
    type Error;
    type Result;
    async fn run(self) -> Self::Result;
}

#[derive(Clone, Eq, PartialEq, Debug)]
pub enum ActorState {
    Stopped,
    Started,
    Running,
    Stopping,
    Shuttingdown,
}

// marker trait
pub trait Event {}

#[async_trait::async_trait]
pub trait SupervisedActor<T: Event + Send>: Actor<T> {}
