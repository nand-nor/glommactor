//! Demonstrates creation of a supervised actor and handle. In this example
//! the supervisor and actor-handle are pinned to core 1 and the actor is
//! pinned to core 0. The handle sends a hello_world request, then the
//! supervisor requests the state and sends a shutdown message to the actor
//!
//! Note that the supervisor can run as an actor and be pinned to it's
//! own core, see supervisor_core example
use std::time::Duration;

use glommactor::{
    handle::SupervisedActorHandle, spawn_exec_actor, spawn_exec_handle_fut, Actor, ActorError,
    ActorId, ActorState, Event, SupervisedActor, SupervisorHandle, SupervisorMessage,
};
use glommio::{Latency, Placement};
use tracing::Level;
use tracing_subscriber::FmtSubscriber;

pub type Reply = flume::Sender<()>;

#[derive(Clone, Debug)]
pub enum HelloWorldEvent {
    SayHello { reply: Reply },
}

impl Event for HelloWorldEvent {}

struct HelloWorldActor {
    receiver: flume::Receiver<HelloWorldEvent>,
    supervisor: flume::Receiver<SupervisorMessage>,
    state: ActorState,
    id: ActorId,
}

impl HelloWorldActor {
    fn new(
        receiver: flume::Receiver<HelloWorldEvent>,
        supervisor: flume::Receiver<SupervisorMessage>,
    ) -> Self {
        Self {
            receiver,
            supervisor,
            state: ActorState::Started,
            id: 0,
        }
    }
}

struct HandleWrapper {
    handle: SupervisedActorHandle<HelloWorldEvent>,
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

#[async_trait::async_trait]
impl SupervisedActor<HelloWorldEvent> for HelloWorldActor where HelloWorldEvent: Event + Send {}

impl HelloWorldActor {
    async fn say_hello(&mut self) {
        tracing::info!("Hello, world!");
    }

    async fn set_id(&mut self, id: ActorId) {
        self.id = id;
    }

    async fn event_loop(mut self) -> Result<(), ActorError<HelloWorldEvent>> {
        self.state = ActorState::Running;
        loop {
            futures::select! {
                    event = self.receiver.recv_async() => {
                    match event {
                        Ok(event) => self.process_event(event).await,
                        Err(e) => {
                            tracing::warn!("Channel error {e:}");
                        }
                    };
                },
                event = self.supervisor.recv_async() => {
                    match event {
                        Ok(event) => self.process_supervision(event).await,
                        Err(e) => {
                            tracing::warn!("Supervisor channel error {e:}");
                        }
                    };
                }
            };

            if self.state == ActorState::Shuttingdown {
                break;
            }
        }
        tracing::info!("Actor has shut down");
        Ok(())
    }

    async fn process_supervision(&mut self, msg: SupervisorMessage) {
        tracing::trace!("Processing supervision {msg:?}");

        match msg {
            SupervisorMessage::SetId { id } => {
                self.set_id(id).await;
            }
            SupervisorMessage::Start => {
                self.state = ActorState::Running;
            }
            SupervisorMessage::Stop => {
                self.state = ActorState::Stopped;
            }
            SupervisorMessage::Shutdown => {
                tracing::info!("Shutting down!");
                self.state = ActorState::Shuttingdown;
                // re-assigning the receiver will close all sender ends
                let (_sender, receiver) = flume::unbounded();
                self.receiver = receiver;
            }
            SupervisorMessage::Id { reply } => {
                reply.send(self.id).ok();
            }
            SupervisorMessage::State { reply } => {
                reply.send(self.state.clone()).ok();
            }
        }
    }

    async fn process_event(&mut self, event: HelloWorldEvent) {
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
        }
    }
}

fn main() -> Result<(), ActorError<HelloWorldEvent>> {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::TRACE)
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");

    let mut handle_vec = vec![];

    // create supervisor
    let (mut supervisor, supervisor_handle) = SupervisorHandle::new();
    // direct access via supervisor actor (for operations outside of async context)
    let (tx, rx, id) = supervisor
        .new_supervisee_setup()
        .expect("Unable to alloc new channels for supervisee");

    // create a supervised actor and handle before running in local executor tasks
    let (actor, handle) = SupervisedActorHandle::new(tx, rx, id, HelloWorldActor::new);

    let handle_wrapper = HandleWrapper { handle };

    // run actor on core 0
    handle_vec.push(
        spawn_exec_actor(
            actor,
            1000,
            Latency::Matters(Duration::from_millis(500)),
            Placement::Fixed(0),
            "rt-actor",
            "tq-actor",
        )
        .expect("Unable to spawn actor onto runtime"),
    );

    // run supervisor actor on core 1, give more shares and set
    // latency to matter for the supervisor actor task
    // (needed because we will also run the handle future
    // on the same core in this example)
    handle_vec.push(
        spawn_exec_actor(
            supervisor,
            10000,
            Latency::Matters(Duration::from_millis(100)),
            Placement::Fixed(1),
            "rt-supervisor",
            "tq-supervisor",
        )
        .expect("Unable to spawn actor onto runtime"),
    );

    // In this future, the handle to the actor sends a request
    // and then the supervisor handle checks state then shuts down the
    // actor (oob from the direct actor handle)
    let fut = async move {
        if let Err(e) = handle_wrapper.say_hello().await {
            tracing::error!("Failed to send say hello {e:}");
        } else {
            tracing::info!("Sent say hello request");
        }

        let state = supervisor_handle
            .actor_state(id)
            .await
            .map_err(|e| {
                tracing::error!("Failed to send shutdown actor action {e:}");
            })
            .ok();

        if let Some(state) = state {
            tracing::info!("Actor state for id {id:} is {state:?}");
        }

        tracing::info!("Supervisor sending shutdown request");
        supervisor_handle
            .shutdown_actor(id)
            .await
            .map_err(|e| {
                tracing::error!("Failed to send shutdown actor action {e:}");
            })
            .ok();
    };

    // pin handle to actor to core 1 along with supervisor, give
    // it fewer shares and set latency to not matter
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
