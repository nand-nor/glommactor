//! A very simple actor framework, built for the glommio runtime

mod actor;
pub mod channel;
mod error;
pub mod handle;
mod supervisor;

pub use actor::{Actor, ActorState, Event, PriorityActor, PriorityEvent, SupervisedActor};
pub use channel::{ChannelRx, ChannelTx, PriorityChannelRx, PriorityRx};
pub use error::ActorError;
pub use supervisor::{Supervision, Supervisor, SupervisorHandle, SupervisorMessage};

pub type ActorId = u16;

pub type PriorityActorHandle<T> = handle::ActorHandle<
    T,
    async_priority_channel::Sender<T, PriorityEvent>,
    async_priority_channel::Receiver<T, PriorityEvent>,
>;

pub fn new_priority_actor_with_handle<T: Event + Send, A: Actor<T> + Sized + Unpin + 'static>(
    constructor: impl FnOnce(async_priority_channel::Receiver<T, PriorityEvent>) -> A,
) -> (A, PriorityActorHandle<T>) {
    let (tx, rx) = async_priority_channel::unbounded();
    // create actor and handle before running in local executor tasks
    handle::ActorHandle::new(constructor, tx, rx)
}

#[cfg(test)]
mod tests {
    use super::handle::ActorHandle;
    use super::*;
    use crate::handle::Handle;

    #[test]
    fn single_event() {
        pub type Reply<T> = flume::Sender<T>;

        pub enum HelloEvent {
            SayHello { reply: Reply<()> },
        }
        impl Event for HelloEvent {}
        struct HelloActor {
            receiver: flume::Receiver<HelloEvent>,
        }

        impl HelloActor {
            fn new(receiver: flume::Receiver<HelloEvent>) -> Self {
                Self { receiver }
            }
            async fn say_hello(&mut self) {
                println!("Hello hi hey!!");
            }

            async fn get_single_event(mut self) -> Result<(), ActorError<HelloEvent>> {
                let HelloEvent::SayHello { reply } = self
                    .receiver
                    .recv_async()
                    .await
                    .expect("Failed to receive event");
                self.say_hello().await;
                reply.send(()).expect("Failed to send reply");

                Ok(())
            }
        }

        #[async_trait::async_trait]
        impl Actor<HelloEvent> for HelloActor
        where
            HelloEvent: Event + Send,
        {
            type Rx = futures::channel::mpsc::Receiver<HelloEvent>;
            type Error = ActorError<HelloEvent>;
            type Result = Result<(), Self::Error>;
            async fn run(self) -> Self::Result {
                self.get_single_event().await
            }
        }

        impl ActorHandle<HelloEvent, flume::Sender<HelloEvent>, flume::Receiver<HelloEvent>> {
            async fn say_hello(&self) -> Result<(), ActorError<HelloEvent>> {
                let (tx, rx) = flume::bounded(1);
                let msg = HelloEvent::SayHello { reply: tx };
                self.send(msg)
                    .await
                    .expect("Faied to send from actor handle");
                rx.recv_async()
                    .await
                    .expect("Failed to recv from actor handle");
                Ok(())
            }
        }

        let (sender, receiver) = flume::unbounded();

        let (actor, handle) = ActorHandle::new(HelloActor::new, sender, receiver);

        let handle = glommio::LocalExecutorBuilder::new(glommio::Placement::Fixed(0))
            .name(&format!("{}{}", "test-handle", 0))
            .spawn(move || async move {
                let tq = glommio::executor().create_task_queue(
                    glommio::Shares::default(),
                    glommio::Latency::NotImportant,
                    "test",
                );

                let task = glommio::spawn_local_into(actor.run(), tq)
                    .map(|t| t.detach())
                    .expect("Failed to spawn detached task");
                handle.say_hello().await.expect("Failed to say hello");
                task.await;
            })
            .expect("Failed to execute");

        handle.join().expect("Failed to join glommio join handle");
    }
}
