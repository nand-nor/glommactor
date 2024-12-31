//! Demonstrates the creation of a supervisor handle that is pinned to a
//! specific core and runs a heartbeat supervision task, configured
//! to check the state of each registered actor on a fixed interval, and
//! execute a recovery task if heartbeat timeout scenario is encountered

use glommactor::{
    handle::SupervisedActorHandle, Actor, ActorError, ActorId, ActorState, Event, SupervisedActor,
    Supervision, SupervisorHandle, SupervisorMessage,
};
use glommio::{executor, Latency, LocalExecutorBuilder, Placement, Shares};
use std::time::Duration;
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

                        if self.state == ActorState::Stopped {
                            // dont process events if in stopped state
                        } else {
                            match event {
                                Ok(event) => self.process_event(event).await,
                                Err(e) => {
                                    tracing::warn!("Channel error {e:}");
                                }
                            };
                        }
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
                tracing::info!("Actor is restarting!");
                self.state = ActorState::Running;
            }
            SupervisorMessage::Stop => {
                self.state = ActorState::Stopped;
            }
            SupervisorMessage::ClearAll => todo!(),
            SupervisorMessage::Shutdown => {
                tracing::info!("Shutting down!");
                self.state = ActorState::Shuttingdown;
                // re-assigning the receiver will close all sender ends
                let (_sender, receiver) = flume::unbounded();
                self.receiver = receiver;
            }
            SupervisorMessage::Backup => todo!(),
            SupervisorMessage::Id { reply } => {
                reply.send(self.id).ok();
            }
            SupervisorMessage::State { reply } => {
                // heartbeat checks call for actor to report their state,
                // and in this example we stop the actor to demonstrate how
                // supervisor task can restart it. So, simulate heartbeat
                // failure to response when in stopped state
                if self.state != ActorState::Stopped {
                    reply.send(self.state.clone()).ok();
                }
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
                reply.send(self.say_hello().await).ok();
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

    // create supervisor actor and handle
    let (mut supervisor, supervisor_handle) = SupervisorHandle::new();

    // [Supervisor] struct allows direct access via supervisor-as-actor,
    // which can be usef to set up new actor outside of async runtime context
    // since all actor handle operations are async
    let (tx, rx, id) = supervisor.new_supervisee_setup().unwrap();

    // create a supervised actor and handle before running/spawning in local executor tasks
    let (actor, handle) = SupervisedActorHandle::new(tx, rx, id, HelloWorldActor::new);

    // clone supervisor handle for use in demonstrating response to suspend
    // via heartbeat recovery func
    let supervised_handle = supervisor_handle.clone();
    let handle_wrapper = HandleWrapper { handle };

    // pin actor to core 0
    handle_vec.push(
        LocalExecutorBuilder::new(Placement::Fixed(0))
            .name(&format!("{}{}", "rt-actor", 0))
            .spawn(move || async move {
                let tq = executor().create_task_queue(
                    Shares::default(),
                    Latency::Matters(Duration::from_millis(500)),
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
                    if let Err(e) = handle_wrapper.say_hello().await {
                        tracing::error!("Failed to send say hello {e:}");
                    } else {
                        tracing::info!("Sent say hello request");
                    }

                    glommio::timer::sleep(Duration::from_secs(5)).await;
                    tracing::info!("Supervisor suspending actor... (demonstrating restart!)");
                    supervised_handle.suspend_actor(id).await.ok();
                };

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

    // pin supervisor to core 2
    handle_vec.push(
        LocalExecutorBuilder::new(Placement::Fixed(2))
            .name(&format!("{}{}", "rt-supervisor", 0))
            .spawn(move || async move {
                let tq = executor().create_task_queue(
                    Shares::default(),
                    Latency::Matters(Duration::from_millis(1)),
                    "supervisor-tq",
                );

                // create separate task queue for spawning supervisor actor
                let stq = executor().create_task_queue(
                    Shares::default(),
                    Latency::Matters(Duration::from_millis(1)),
                    "supervisor-tq-run",
                );
                glommio::spawn_local_into(supervisor.run(), stq)
                    .map(|t| t.detach())
                    .map_err(|e| {
                        tracing::error!("Error spawning task for handle {e:}");
                        panic!("handle core panic");
                    })
                    .ok();

                let (shutdown_notif_tx, shutdown_notif_rx) = flume::bounded(1);

                // this future represents the top-level supervision task
                // that performs heartbeat checks. An example closure
                // is passed in to [Supervision] struct to demonstrate
                // how message passing can be used to alert subscribers if
                // an actor has shut down, and optioanlly restart them.
                // The function provides 1 generic optional arg, so
                // caller can supply a variety of function args and
                // even pass in a closure that does nothing.
                // In this example, the closure is used for
                // restarting the stopped actor
                let monitor_task_handle = supervisor_handle.clone();
                let fut = async move {
                    loop {
                        glommio::timer::sleep(Duration::from_secs(2)).await;
                        let supervision = Supervision::new(
                            monitor_task_handle.clone(),
                            |id, shutdown_notif_tx: Option<flume::Sender<ActorId>>| {
                                tracing::info!(
                                    "Example closure running when heartbeat detection fails! {:?}",
                                    id
                                );
                                if let Some(tx) = shutdown_notif_tx {
                                    tx.send(id).ok();
                                }
                            },
                            Some(shutdown_notif_tx.clone()),
                        );
                        supervision.await;
                    }
                };

                let restart_task_handle = supervisor_handle.clone();
                let mut task_vec = vec![];
                let shutdown_task = glommio::spawn_local_into(
                    async move {
                        if let Ok(id) = shutdown_notif_rx.recv_async().await.map_err(|e| {
                            let msg = format!("Send cancelled {e:}");
                            tracing::error!(msg);
                        }) {
                            tracing::info!("heartbeat checker func fired! Restarting...");
                            restart_task_handle.start_actor(id).await.ok();
                        }
                    },
                    tq,
                )
                .map(|t| t.detach())
                .map_err(|e| {
                    tracing::error!("Error spawning task for handle {e:}");
                    panic!("handle core panic");
                })
                .ok();

                if let Some(shutdown_task) = shutdown_task {
                    task_vec.push(shutdown_task);
                }

                let task = glommio::spawn_local_into(fut, tq)
                    .map(|t| t.detach())
                    .map_err(|e| {
                        tracing::error!("Error spawning task for handle {e:}");
                        panic!("handle core panic");
                    })
                    .ok();
                if let Some(task) = task {
                    task_vec.push(task);
                }

                futures::future::join_all(task_vec).await;
            })
            .unwrap(),
    );

    for handle in handle_vec {
        handle.join().unwrap();
    }

    Ok(())
}
