//! Simple demonstration of the Actor trait and corresponding handle
//! as implemented for a HelloWorldActor
use glommactor::{
    handle::{ActorHandle, Handle},
    spawn_exec_actor, spawn_exec_handle_fut, Actor, ActorError, ActorState, Event,
};
use glommio::{Latency, Placement};
use std::time::Duration;
use tracing::Level;
use tracing_subscriber::FmtSubscriber;

pub type Reply<T> = flume::Sender<T>;

#[derive(Clone, Debug)]
pub enum HelloWorldEvent {
    SayHello { reply: Reply<()> },
    Stop,
    Start,
    Shutdown,
    State { reply: Reply<ActorState> },
}

impl Event for HelloWorldEvent {}

struct HelloWorldActor {
    receiver: flume::Receiver<HelloWorldEvent>,
    state: ActorState,
}

impl HelloWorldActor {
    fn new(receiver: flume::Receiver<HelloWorldEvent>) -> Self {
        Self {
            receiver,
            state: ActorState::Started,
        }
    }
}

struct HandleWrapper {
    handle: ActorHandle<
        HelloWorldEvent,
        flume::Sender<HelloWorldEvent>,
        flume::Receiver<HelloWorldEvent>,
    >,
}

#[async_trait::async_trait]
impl Handle<HelloWorldEvent> for HandleWrapper {
    type State = ActorState;
    type Result = <ActorHandle<
        HelloWorldEvent,
        flume::Sender<HelloWorldEvent>,
        flume::Receiver<HelloWorldEvent>,
    > as Handle<HelloWorldEvent>>::Result;
    async fn send(&self, event: HelloWorldEvent) -> Self::Result {
        self.handle.send(event).await
    }
}

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
        self.handle.send(msg).await.ok();

        rx.recv_async().await.map_err(|e| {
            let msg = format!("Send cancelled {e:}");
            tracing::error!(msg);
            ActorError::ActorError(msg)
        })?;

        Ok(())
    }

    async fn stop(&self) -> Result<(), ActorError<HelloWorldEvent>> {
        let msg = HelloWorldEvent::Stop;
        let _ = self.handle.send(msg).await;
        Ok(())
    }

    async fn shutdown(&self) -> Result<(), ActorError<HelloWorldEvent>> {
        let msg = HelloWorldEvent::Shutdown;
        let _ = self.handle.send(msg).await;
        Ok(())
    }

    async fn state(
        &self,
    ) -> Result<<Self as Handle<HelloWorldEvent>>::State, ActorError<HelloWorldEvent>> {
        let (tx, rx) = flume::bounded(1);

        let msg = HelloWorldEvent::State { reply: tx };
        let _ = self.handle.send(msg).await;
        rx.recv_async().await.map_err(|e| {
            let msg = format!("Send cancelled {e:}");
            tracing::error!(msg);
            ActorError::ActorError(msg)
        })
    }
}

#[async_trait::async_trait]
impl Actor<HelloWorldEvent> for HelloWorldActor
where
    HelloWorldEvent: Event + Send,
{
    type Rx = futures::channel::mpsc::Receiver<HelloWorldEvent>;
    type Error = ActorError<HelloWorldEvent>;
    type Result = Result<(), Self::Error>;
    async fn run(self) -> Self::Result {
        self.event_loop().await
    }
}

impl HelloWorldActor {
    async fn say_hello(&mut self) {
        tracing::info!("Hello, world!");
    }

    async fn event_loop(mut self) -> Result<(), ActorError<HelloWorldEvent>> {
        self.state = ActorState::Running;
        while let Ok(event) = self.receiver.recv_async().await {
            self.process(event).await;
            if self.state == ActorState::Stopping {
                break;
            }
        }
        self.state = ActorState::Stopped;
        Ok(())
    }

    async fn process(&mut self, event: HelloWorldEvent) {
        tracing::trace!("Processing event {event:?}");

        match event {
            HelloWorldEvent::SayHello { reply } => {
                if self.state == ActorState::Stopped {
                    drop(reply);
                    return;
                }
                {
                    self.say_hello().await;
                    reply.send(())
                }
                .ok();
            }
            HelloWorldEvent::Start => {
                self.state = ActorState::Running;
            }
            HelloWorldEvent::Stop => {
                tracing::info!("Stopping!");
                self.state = ActorState::Stopped;
            }
            HelloWorldEvent::Shutdown => {
                tracing::info!("Shutting down!");
                self.state = ActorState::Stopping;
                // re-assigning the receiver will close all sender ends
                let (_sender, receiver) = flume::unbounded();
                self.receiver = receiver;
            }
            HelloWorldEvent::State { reply } => {
                reply.send(self.state.clone()).ok();
            }
        }
        tracing::debug!("Processed");
    }
}

fn main() -> Result<(), ActorError<HelloWorldEvent>> {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::TRACE)
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");

    let mut handle_vec = vec![];

    let (tx, rx) = flume::unbounded();
    // create actor and handle before running in local executor tasks
    let (actor, handle) = ActorHandle::new(HelloWorldActor::new, tx, rx);
    let handle_wrapper = HandleWrapper { handle };

    // pin actor to core 0
    handle_vec.push(
        spawn_exec_actor(
            actor,
            100,
            Latency::Matters(Duration::from_millis(1)),
            Placement::Fixed(0),
            "rt-actor",
            "tq-actor",
        )
        .expect("Unable to spawn actor onto runtime"),
    );

    // define a future for the handle spawner function to execute
    let fut = async move {
        handle_wrapper.say_hello().await.ok();
        tracing::info!("Sent say hello request");

        if let Ok(state) = handle_wrapper.state().await {
            tracing::info!("Actor state is {:?}", state);
        }

        handle_wrapper.stop().await.ok();
        tracing::info!("Sent stop request");

        if let Ok(state) = handle_wrapper.state().await {
            tracing::info!("Actor state is {:?}", state);
        }

        // Expect this to fail due to how the actor is using state to
        // drop certain requests (see line 132)
        handle_wrapper
            .say_hello()
            .await
            .expect_err("Actor still responded to say_hello despite being stopped");

        // without this call, because we cloned the handle above, the program would never terminate
        handle_wrapper.shutdown().await.ok();
        tracing::info!("Sent shutdown request");
    };

    // pins future where handle to actor is operating to core 1
    handle_vec.push(
        spawn_exec_handle_fut(
            100,
            Latency::NotImportant,
            Placement::Fixed(1),
            "rt-handle",
            "tq-handle",
            fut,
        )
        .expect("Unable to spawn actor onto runtime"),
    );

    for handle in handle_vec {
        handle.join().unwrap();
    }

    Ok(())
}
