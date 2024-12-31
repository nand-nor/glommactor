//! Demonstrates creation of a supervised actor and handle. In this example
//! the supervisor and actor-handle are pinned to core 1 and the actor is
//! pinned to core 0. The handle sends a hello_world request, then the
//! supervisor requests the state and sends a shutdown message to the actor
//!
//! Note that the supervisor can run as an actor and be pinned to it's
//! own core, see supervisor_core example
use glommactor::{
    handle::SupervisedActorHandle, Actor, ActorError, ActorId, ActorState, Event, SupervisedActor,
    SupervisorHandle, SupervisorMessage,
};
use glommio::{executor, Latency, LocalExecutorBuilder, Placement, Shares};
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
                }.ok();
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
    let (tx, rx, id) = supervisor.new_supervisee_setup().unwrap();

    // create a supervised actor and handle before running in local executor tasks
    let (actor, handle) = SupervisedActorHandle::new(tx, rx, id, HelloWorldActor::new);

    let handle_wrapper = HandleWrapper { handle };

    // pin actor to core 0
    handle_vec.push(
        LocalExecutorBuilder::new(Placement::Fixed(0))
            .name(&format!("{}{}", "rt-actor", 0))
            .spawn(move || async move {
                let tq = executor().create_task_queue(
                    Shares::default(),
                    Latency::NotImportant,
                    "actor-tq",
                );

                let task = glommio::spawn_local_into(actor.run(), tq)
                    .map(|t| t.detach())
                    .map_err(|e| {
                        tracing::error!("Error spawning actor {e:}");
                        panic!("Actor core panic");
                    })
                    .ok();
                if let Some(task) = task {
                    task.await;
                }
            })
            .unwrap(),
    );

    // pin handle to actor and actor supervisor to core 1
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

                glommio::spawn_local_into(supervisor.run(), tq)
                    .map(|t| t.detach())
                    .map_err(|e| {
                        tracing::error!("Error spawning task for handle {e:}");
                        panic!("handle core panic");
                    })
                    .ok();

                let task = glommio::spawn_local_into(fut, tq)
                    .map(|t| t.detach())
                    .map_err(|e| {
                        tracing::error!("Error spawning task for handle {e:}");
                        panic!("handle core panic");
                    })
                    .ok();
                if let Some(task) = task {
                    task.await;
                }
            })
            .unwrap(),
    );

    for handle in handle_vec {
        handle.join().unwrap();
    }

    Ok(())
}
