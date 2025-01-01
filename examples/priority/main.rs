//! Simple demonstration of the Actor trait and corresponding handle
//! as implemented for a HelloWorldActor
use glommactor::{
    handle::ActorHandle, new_priority_actor_with_handle, Actor, ActorError, Event, PriorityActor,
    PriorityEvent, PriorityRx,
};
use glommio::{executor, Latency, LocalExecutorBuilder, Placement, Shares};
use std::time::Duration;
use tracing::Level;
use tracing_subscriber::FmtSubscriber;

pub type Reply<T> = flume::Sender<T>;

#[derive(Clone, Debug)]
pub enum HelloWorldEvent {
    SayHello { reply: Reply<()> },
    Shutdown,
}

impl Event for HelloWorldEvent {}

struct HelloWorldActor {
    receiver: PriorityRx<HelloWorldEvent>,
}

#[async_trait::async_trait]
impl Actor<HelloWorldEvent> for HelloWorldActor
where
    HelloWorldEvent: Event + Send,
{
    type Rx = PriorityRx<HelloWorldEvent>;
    type Error = ActorError<HelloWorldEvent>;
    type Result = Result<(), Self::Error>;
    async fn run(self) -> Self::Result {
        self.event_loop().await
    }
}

#[async_trait::async_trait]
impl PriorityActor<HelloWorldEvent> for HelloWorldActor
where
    HelloWorldEvent: Event + Send,
{
    type Rx = PriorityRx<HelloWorldEvent>;
}

impl HelloWorldActor {
    fn new(receiver: PriorityRx<HelloWorldEvent>) -> Self {
        Self { receiver }
    }
}

struct HandleWrapper {
    handle: ActorHandle<
        HelloWorldEvent,
        async_priority_channel::Sender<HelloWorldEvent, PriorityEvent>,
        async_priority_channel::Receiver<HelloWorldEvent, PriorityEvent>,
    >,
}

use glommactor::handle::PriorityHandle;

impl Clone for HandleWrapper {
    fn clone(&self) -> Self {
        Self {
            handle: self.handle.clone(),
        }
    }
}

impl HandleWrapper {
    async fn say_hello(&self) -> Result<(), ActorError<HelloWorldEvent>> {
        let (tx, rx) = flume::bounded(1);
        let msg = HelloWorldEvent::SayHello { reply: tx };
        self.handle
            .send_with_priority(msg, PriorityEvent::RealTime)
            .await
            .ok();

        rx.recv_async().await.map_err(|e| {
            let msg = format!("Send cancelled {e:}");
            tracing::error!(msg);
            ActorError::ActorError(msg)
        })?;

        Ok(())
    }

    async fn shutdown(&self) -> Result<(), ActorError<HelloWorldEvent>> {
        let msg = HelloWorldEvent::Shutdown;
        let _ = self
            .handle
            .send_with_priority(msg, PriorityEvent::RealTime)
            .await;
        Ok(())
    }
}

impl HelloWorldActor {
    async fn say_hello(&mut self, priority: PriorityEvent) {
        tracing::info!("Hey this is a {:?} priority message!!", priority);
    }

    async fn event_loop(mut self) -> Result<(), ActorError<HelloWorldEvent>> {
        loop {
            match self.receiver.recv().await {
                Ok((event, priority)) => self.process(event, priority).await,
                Err(e) => {
                    tracing::warn!("Channel error {e:}");
                    break;
                }
            }
        }
        Ok(())
    }

    async fn process(&mut self, event: HelloWorldEvent, priority: PriorityEvent) {
        tracing::trace!("Processing event {event:?}");

        match event {
            HelloWorldEvent::SayHello { reply } => {
                {
                    self.say_hello(priority).await;
                    reply.send(())
                }
                .ok();
            }
            HelloWorldEvent::Shutdown => {
                tracing::info!("Shutting down!");
                let (_sender, receiver) = async_priority_channel::unbounded();
                self.receiver = receiver;
            }
        }
    }
}

fn main() -> Result<(), ActorError<HelloWorldEvent>> {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::TRACE)
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");

    let mut handle_vec = vec![];

    // create actor and handle before running in local executor tasks
    let (actor, handle) = new_priority_actor_with_handle(HelloWorldActor::new);
    let handle_wrapper = HandleWrapper { handle };
    // without the call to shutdown done at line ~194, because we clone the handle,
    // this would keep the recieve end of the actor channel open
    let _handle_clone = handle_wrapper.clone();

    // pin actor to core 0
    handle_vec.push(
        LocalExecutorBuilder::new(Placement::Fixed(0))
            .name(&format!("{}{}", "rt-actor", 0))
            .spawn(move || async move {
                let tq = executor().create_task_queue(
                    Shares::default(),
                    Latency::Matters(Duration::from_millis(1)),
                    "actor-tq",
                );

                let task = glommio::spawn_local_into(actor.run(), tq)
                    .map(|t| t.detach())
                    .map_err(|e| {
                        tracing::error!("Error spawning actor {e:}");
                        panic!("Actor core panic");
                    })
                    .unwrap();
                task.await;
            })
            .unwrap(),
    );

    // pin handle to actor to core 1
    handle_vec.push(
        LocalExecutorBuilder::new(Placement::Fixed(1))
            .name(&format!("{}{}", "rt-handle", 0))
            .spawn(move || async move {
                let tq = executor().create_task_queue(
                    Shares::default(),
                    Latency::NotImportant,
                    "handle-tq",
                );
                let fut = async move {
                    handle_wrapper.say_hello().await.ok();
                    tracing::info!("Sent say hello request");

                    // without this call, because we cloned the handle above, the program would never terminate
                    handle_wrapper.shutdown().await.ok();
                    tracing::info!("Sent shutdown request");
                };

                let task = glommio::spawn_local_into(fut, tq)
                    .map(|t| t.detach())
                    .map_err(|e| {
                        tracing::error!("Error spawning task for handle {e:}");
                        panic!("handle core panic");
                    })
                    .unwrap();
                task.await;
            })
            .unwrap(),
    );

    for handle in handle_vec {
        handle.join().unwrap();
    }

    Ok(())
}
