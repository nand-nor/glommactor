use crate::channel::PriorityChannelRx;

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

#[derive(Clone, Debug, Ord, PartialOrd, Default)]
pub enum PriorityEvent {
    BestEffort, // effectively broadcast, but may be delayed indefinitely (no delivery guarantee)
    #[default]
    Low, // guaranteed to be delayed only a few times? need to determine policy
    High,       // guaranteed to be  elayed once, then bumped in priority
    RealTime,   // highest priority, guaranteed delivery
}

// Only care about the priorities being equal, not the inner
// events
impl PartialEq for PriorityEvent {
    fn eq(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (Self::BestEffort, Self::BestEffort)
                | (Self::Low, Self::Low)
                | (Self::High, Self::High)
                | (Self::RealTime, Self::RealTime)
        )
    }
}

impl Eq for PriorityEvent {}

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

                let task = glommio::spawn_local_into(actor.run(), tq)
                    .map(|t| t.detach())
                    .expect("Failed to spawn detached task");

                // Send multiple messages simultaneously, we expect highest priority to be processed first
                tx.send((), PriorityEvent::BestEffort)
                    .await
                    .expect("Faied to send from actor handle");
                tx.send((), PriorityEvent::Low)
                    .await
                    .expect("Faied to send from actor handle");
                tx.send((), PriorityEvent::High)
                    .await
                    .expect("Faied to send from actor handle");
                tx.send((), PriorityEvent::RealTime)
                    .await
                    .expect("Faied to send from actor handle");
                task.await;
            })
            .expect("Failed to execute");

        handle.join().expect("Failed to join glommio join handle");

        // Of the multiple messages sent, assert the highest priority is processed first
        assert_eq!(
            test_rx.recv().expect("Unable to recv test result"),
            PriorityEvent::RealTime
        );
    }

    #[test]
    fn ord() {
        assert_eq!(
            PriorityEvent::BestEffort.cmp(&PriorityEvent::Low),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            PriorityEvent::Low.cmp(&PriorityEvent::RealTime),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            PriorityEvent::RealTime.cmp(&PriorityEvent::High),
            std::cmp::Ordering::Greater
        );
    }
}
