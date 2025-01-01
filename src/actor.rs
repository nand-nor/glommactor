use crate::channel::PriorityChannelRx;

/// Simple actor trait
#[async_trait::async_trait]
pub trait Actor<T: Event + Send>: Send {
    type Rx;
    type Error;
    type Result;
    /// The actor's [`Actor::run`] method is essential the actor
    /// spawn point
    async fn run(self) -> Self::Result;
}

/// [`ActorState`] is the default state enum available for
/// use by [`Actor<T>`]s and their handles, as needed. It is
/// fixed for use by the [`crate::supervisor::Supervisor`]  
/// actor, so supervised actors must use this enum to
/// represent state
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

/// Represents a trait for supervised [`Actor`]s
#[async_trait::async_trait]
pub trait SupervisedActor<T: Event + Send>: Actor<T> {}

/// Actors implementing the [`PriorityActor`] trait
/// use the [`PriorityEvent`] enum
/// to send events
#[derive(Clone, Debug, Ord, PartialOrd, Default)]
pub enum PriorityEvent {
    Low,
    #[default]
    Medium,
    High,
    Urgent,
}

impl PartialEq for PriorityEvent {
    fn eq(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (Self::Urgent, Self::Urgent)
                | (Self::Low, Self::Low)
                | (Self::Medium, Self::Medium)
                | (Self::High, Self::High)
        )
    }
}

impl Eq for PriorityEvent {}

/// [`PriorityActor`]s represent an actor that is expected to use
/// [`async_priority_channel`] objects for sending and receiving events
/// between actors & their handles. The channels use the [`PriorityEvent`]
/// enum to send events with specific priorities
#[async_trait::async_trait]
pub trait PriorityActor<T: Event + Send>: Actor<T>
where
    Self: Sized,
{
    type Rx: PriorityChannelRx<T>;
    async fn run(self) -> <Self as Actor<T>>::Result {
        <Self as Actor<T>>::run(self).await
    }
}

#[cfg(test)]
mod tests {
    use crate::channel::PriorityRx;

    use super::*;
    impl Event for () {}

    #[test]
    fn priority_event() {
        struct HelloActor {
            receiver: PriorityRx<()>,
            test_result: flume::Sender<PriorityEvent>,
        }

        impl HelloActor {
            fn new(receiver: PriorityRx<()>, test_result: flume::Sender<PriorityEvent>) -> Self {
                Self {
                    receiver,
                    test_result,
                }
            }
            async fn say_hello(&mut self, priority: PriorityEvent) {
                println!("Hello method received {:?} priority message!!", priority);
                self.test_result
                    .send_async(priority)
                    .await
                    .expect("Failed to send result");
            }

            async fn get_event(mut self) -> Result<(), crate::error::ActorError<()>> {
                let input = self.receiver.recv().await.expect("Failed to receive event");
                self.say_hello(input.1).await;

                Ok(())
            }
        }

        #[async_trait::async_trait]
        impl Actor<()> for HelloActor
        where
            (): Event + Send,
        {
            type Rx = PriorityRx<()>;
            type Error = crate::error::ActorError<()>;
            type Result = Result<(), Self::Error>;
            async fn run(self) -> Self::Result {
                self.get_event().await
            }
        }

        #[async_trait::async_trait]
        impl PriorityActor<()> for HelloActor
        where
            (): Event + Send,
        {
            type Rx = PriorityRx<()>;
        }

        let (tx, rx) = async_priority_channel::bounded::<(), PriorityEvent>(10);
        let (test_sender, test_rx) = flume::unbounded();

        // dont need a handle for this test
        let actor = HelloActor::new(rx, test_sender);

        let handle = glommio::LocalExecutorBuilder::new(glommio::Placement::Fixed(0))
            .name(&format!("{}{}", "test-handle", 0))
            .spawn(move || async move {
                let tq = glommio::executor().create_task_queue(
                    glommio::Shares::default(),
                    glommio::Latency::NotImportant,
                    "test",
                );

                let task = glommio::spawn_local_into(PriorityActor::run(actor), tq)
                    .map(|t| t.detach())
                    .expect("Failed to spawn detached task");

                // Send multiple messages simultaneously, we expect highest priority to be processed first
                tx.send((), PriorityEvent::Low)
                    .await
                    .expect("Faied to send from actor handle");
                tx.send((), PriorityEvent::Medium)
                    .await
                    .expect("Faied to send from actor handle");
                tx.send((), PriorityEvent::High)
                    .await
                    .expect("Faied to send from actor handle");
                tx.send((), PriorityEvent::Urgent)
                    .await
                    .expect("Faied to send from actor handle");
                task.await;
            })
            .expect("Failed to execute");

        handle.join().expect("Failed to join glommio join handle");

        // Of the multiple messages sent, assert the highest priority is processed first
        assert_eq!(
            test_rx.recv().expect("Unable to recv test result"),
            PriorityEvent::Urgent
        );
    }

    #[test]
    fn ord() {
        assert_eq!(
            PriorityEvent::Urgent.cmp(&PriorityEvent::Low),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            PriorityEvent::Low.cmp(&PriorityEvent::High),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            PriorityEvent::High.cmp(&PriorityEvent::Medium),
            std::cmp::Ordering::Greater
        );
    }
}
