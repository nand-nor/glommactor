use super::{Actor, ActorError, Event};

#[derive(Clone)]
pub struct ActorHandle<T: Event + Send> {
    sender: flume::Sender<T>,
}

impl<T: Event + Send> ActorHandle<T> {
    pub fn new<A: Actor<T> + Sized + Unpin + 'static>(
        constructor: impl FnOnce(flume::Receiver<T>) -> A,
    ) -> (A, Self) {
        let (sender, receiver) = flume::unbounded();
        let actor = constructor(receiver);

        (actor, Self { sender })
    }

    pub async fn send(&self, event: T) -> Result<(), ActorError<T>> {
        tracing::trace!("Sending event from handle to actor");
        self.sender
            .send_async(event)
            .await
            .map_err(ActorError::from)
    }
}
