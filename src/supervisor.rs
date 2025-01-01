use std::{
    collections::{HashMap, HashSet},
    pin::Pin,
};

use crate::{
    handle::{ActorHandle, Handle},
    Actor, ActorError, ActorId, ActorState, Event,
};
use futures::FutureExt;

pub type Reply<T> = flume::Sender<T>;
pub type SupervisorActionReply<T> = Reply<Result<T>>;
pub type Result<T> = std::result::Result<T, ActorError<SupervisorAction>>;

/// SupervisorMessage enum represents messages sent from
/// the supervisor to supervised actors, either directly
/// or via their handles
#[derive(Clone, Debug)]
pub enum SupervisorMessage {
    SetId { id: ActorId },
    Id { reply: Reply<ActorId> },
    Start,
    Stop,
    Shutdown,
    State { reply: Reply<ActorState> },
}

pub struct Supervisor {
    /// Stores sender end for SupervisorMessages by Actor ID
    handles: HashMap<ActorId, flume::Sender<SupervisorMessage>>,
    /// Used to provide actors with unique IDs
    ids: HashSet<ActorId>,
    /// Supervisor can also optionally act as an actor
    /// but is not required to do so
    receiver: flume::Receiver<SupervisorAction>,
}

impl Supervisor {
    pub fn new(receiver: flume::Receiver<SupervisorAction>) -> Self {
        let mut ids = HashSet::default();
        ids.extend((0..ActorId::MAX).collect::<Vec<_>>());
        Self {
            handles: HashMap::default(),
            ids,
            receiver,
        }
    }

    async fn event_loop(mut self) -> Result<()> {
        let err = loop {
            match self.receiver.recv_async().await {
                Ok(event) => self.process(event).await,
                Err(e) => {
                    tracing::warn!("Channel error {e:}");
                    break e;
                }
            }
        };
        Err(ActorError::from(err))
    }

    async fn process(&mut self, action: SupervisorAction) {
        match action {
            SupervisorAction::NewActorSetup { reply } => {
                reply.send(self.new_supervisee_setup()).ok();
            }
            SupervisorAction::ShutdownAll { reply } => {
                let keys = self.handles.keys().copied().collect::<Vec<_>>();
                for key in keys {
                    self.shutdown_actor(key)
                        .await
                        .map_err(|e| {
                            tracing::error!("Failed to send shutdown to actor {key} {e:}");
                        })
                        .ok();
                }
                reply.send(Ok(())).ok();
            }
            SupervisorAction::Shutdown { id, reply } => {
                reply.send(self.shutdown_actor(id).await).ok();
            }
            SupervisorAction::State { id, reply } => {
                reply.send(self.actor_state(id).await).ok();
            }
            SupervisorAction::GetCurrentIds { reply } => {
                reply.send(self.get_current_ids().await).ok();
            }
            SupervisorAction::Heartbeat { id, reply } => {
                reply.send(self.send_heartbeat(id).await).ok();
            }
            SupervisorAction::Suspend { id, reply } => {
                reply.send(self.stop_actor(id).await).ok();
            }
            SupervisorAction::Start { id, reply } => {
                reply.send(self.start_actor(id).await).ok();
            }
        }
    }

    pub fn new_supervisee_setup(
        &mut self,
    ) -> Result<(
        flume::Sender<SupervisorMessage>,
        flume::Receiver<SupervisorMessage>,
        ActorId,
    )> {
        let (tx, rx) = flume::unbounded();
        let id = *self
            .ids
            .iter()
            .next()
            .ok_or_else(|| ActorError::SystemError("too many actors".to_string()))?;

        self.ids.take(&id);
        let _ = tx.send(SupervisorMessage::SetId { id });
        self.handles.insert(id, tx.clone());
        Ok((tx, rx, id))
    }

    pub async fn shutdown_actor(&mut self, id: ActorId) -> Result<()> {
        tracing::trace!("Supervisor shutting down actor id {id:}");
        if let Some(sender) = self.handles.get(&id) {
            sender
                .send_async(SupervisorMessage::Shutdown)
                .await
                .map_err(|e| {
                    let msg = format!("Send error {e:?}");
                    tracing::error!(msg);
                    ActorError::ActorError(msg)
                })?;
            // replace the ID back in the set
            self.ids.insert(id);
        }
        Ok(())
    }

    pub async fn stop_actor(&mut self, id: ActorId) -> Result<()> {
        tracing::trace!("Supervisor suspending actor id {id:}");
        if let Some(sender) = self.handles.get(&id) {
            sender
                .send_async(SupervisorMessage::Stop)
                .await
                .map_err(|e| {
                    let msg = format!("Send error {e:?}");
                    tracing::error!(msg);
                    ActorError::ActorError(msg)
                })?;
        }
        Ok(())
    }

    pub async fn start_actor(&mut self, id: ActorId) -> Result<()> {
        tracing::trace!("Supervisor (re)starting actor id {id:}");
        if let Some(sender) = self.handles.get(&id) {
            sender
                .send_async(SupervisorMessage::Start)
                .await
                .map_err(|e| {
                    let msg = format!("Send error {e:?}");
                    tracing::error!(msg);
                    ActorError::ActorError(msg)
                })?;
        }
        Ok(())
    }

    async fn actor_state(&mut self, id: ActorId) -> Result<ActorState> {
        tracing::trace!("Supervisor getting actor state id {id:}");
        if let Some(sender) = self.handles.get(&id) {
            let (tx, rx) = flume::bounded(1);
            let msg = SupervisorMessage::State { reply: tx };

            sender.send_async(msg).await.map_err(|e| {
                let msg = format!("Send error {e:?}");
                tracing::error!(msg);
                ActorError::ActorError(msg)
            })?;

            rx.recv_async().await.map_err(|e| {
                let msg = format!("Send cancelled {e:}");
                tracing::error!(msg);
                ActorError::ActorError(msg)
            })
        } else {
            Err(ActorError::SystemError("No such actor ID".to_string()))
        }
    }

    async fn send_heartbeat(&mut self, id: ActorId) -> Result<()> {
        if let Err(e) = self.actor_state(id).await {
            Err(e)
        } else {
            Ok(())
        }
    }

    // to be used in methods that monitor current state of all
    // actors, in order to restart if needed. TODO: configure
    // heartbeat for actors reporting to supervisor
    async fn get_current_ids(&mut self) -> Vec<ActorId> {
        self.handles.keys().copied().collect::<Vec<_>>()
    }
}

/// [SupervisorAction] enum represents messages for interacting
/// with the [Supervisor] via a handle, if it is run as an actor itself.
/// Note that this set of enums is distinct from the [SupervisorMessage] enums
/// and private only to be used by [SupervisorHandle] types directly
/// interfacing with a [Supervisor]-run-as-actor
#[derive(Clone, Debug)]
pub enum SupervisorAction {
    /// shuts down all currently registered supervised actors
    ShutdownAll { reply: SupervisorActionReply<()> },
    /// shuts down a given actor
    Shutdown {
        id: ActorId,
        reply: SupervisorActionReply<()>,
    },
    /// setup data structures for adding new actor
    NewActorSetup {
        reply: SupervisorActionReply<(
            flume::Sender<SupervisorMessage>,
            flume::Receiver<SupervisorMessage>,
            ActorId,
        )>,
    },
    /// returns all currently registered actor IDs
    GetCurrentIds { reply: Reply<Vec<ActorId>> },
    /// get actor state by ID
    State {
        id: ActorId,
        reply: SupervisorActionReply<ActorState>,
    },
    /// performs heartbeat check by ID
    Heartbeat {
        id: ActorId,
        reply: SupervisorActionReply<()>,
    },
    /// Stop the actor from processing events
    Suspend {
        id: ActorId,
        reply: SupervisorActionReply<()>,
    },
    /// Starts or restarts stopped actor
    /// will do nothing if actor is not in stopped state
    Start {
        id: ActorId,
        reply: SupervisorActionReply<()>,
    },
}

/// The [SupervisorHandle] must be used when running the [Supervisor]
/// as an actor, however note that the [Supervisor] can also be a
/// stand-alone entity
#[derive(Clone)]
pub struct SupervisorHandle {
    handle: ActorHandle<
        SupervisorAction,
        flume::Sender<SupervisorAction>,
        flume::Receiver<SupervisorAction>,
    >,
}

impl Event for SupervisorAction {}

impl SupervisorHandle {
    pub fn new() -> (Supervisor, Self) {
        let (tx, rx) = flume::unbounded();
        // create supervisor
        let (supervisor, handle) = ActorHandle::new(Supervisor::new, tx, rx);
        (supervisor, Self { handle })
    }

    pub async fn new_supervisee_setup(
        &self,
    ) -> Result<(
        flume::Sender<SupervisorMessage>,
        flume::Receiver<SupervisorMessage>,
        ActorId,
    )> {
        let (tx, rx) = flume::bounded(1);
        let msg = SupervisorAction::NewActorSetup { reply: tx };
        self.handle.send(msg).await?;

        rx.recv_async().await.map_err(|e| {
            let msg = format!("Send error {e:?}");
            tracing::error!(msg);
            ActorError::ActorError(msg)
        })?
    }

    pub async fn start_actor(&self, id: ActorId) -> Result<()> {
        let (tx, rx) = flume::bounded(1);
        let msg = SupervisorAction::Start { id, reply: tx };
        self.handle.send(msg).await?;

        rx.recv_async().await.map_err(|e| {
            let msg = format!("Send error {e:?}");
            tracing::error!(msg);
            ActorError::ActorError(msg)
        })?
    }

    pub async fn suspend_actor(&self, id: ActorId) -> Result<()> {
        let (tx, rx) = flume::bounded(1);
        let msg = SupervisorAction::Suspend { id, reply: tx };
        self.handle.send(msg).await?;

        rx.recv_async().await.map_err(|e| {
            let msg = format!("Send error {e:?}");
            tracing::error!(msg);
            ActorError::ActorError(msg)
        })?
    }

    pub async fn shutdown_actor(&self, id: ActorId) -> Result<()> {
        let (tx, rx) = flume::bounded(1);
        let msg = SupervisorAction::Shutdown { id, reply: tx };
        self.handle.send(msg).await?;

        rx.recv_async().await.map_err(|e| {
            let msg = format!("Send error {e:?}");
            tracing::error!(msg);
            ActorError::ActorError(msg)
        })?
    }

    pub async fn actor_state(&self, id: ActorId) -> Result<ActorState> {
        let (tx, rx) = flume::bounded(1);
        let msg = SupervisorAction::State { id, reply: tx };
        self.handle.send(msg).await?;

        rx.recv_async().await.map_err(|e| {
            let msg = format!("Send error {e:?}");
            tracing::error!(msg);
            ActorError::ActorError(msg)
        })?
    }

    pub async fn ids(&self) -> Result<Vec<ActorId>> {
        let (tx, rx) = flume::bounded(1);
        let msg = SupervisorAction::GetCurrentIds { reply: tx };
        self.handle.send(msg).await?;

        rx.recv_async().await.map_err(|e| {
            let msg = format!("Send error {e:?}");
            tracing::error!(msg);
            ActorError::ActorError(msg)
        })
    }

    pub async fn heartbeat(&self, id: ActorId) -> Result<()> {
        let (tx, rx) = flume::bounded(1);
        let msg = SupervisorAction::Heartbeat { id, reply: tx };
        self.handle.send(msg).await?;
        let mut fused_timer = glommio::timer::Timer::new(std::time::Duration::from_secs(5)).fuse();
        futures::select! {
            _ = fused_timer => {
                Err(ActorError::HeartbeatTimeout(id))
            }
            res = rx.recv_async() =>  {
                Ok(res.map_err(|e| {
                    let msg = format!("Send error {e:?}");
                    tracing::error!(msg);
                    ActorError::ActorError(msg)
                })??)
            }
        }
    }
}

#[async_trait::async_trait]
impl Actor<SupervisorAction> for Supervisor
where
    SupervisorAction: Event + Send,
{
    type Rx = futures::channel::mpsc::Receiver<SupervisorAction>;
    type Error = ActorError<SupervisorAction>;
    type Result = std::result::Result<(), Self::Error>;
    async fn run(self) -> Self::Result {
        self.event_loop().await
    }
}

pub struct Supervision<B: Send + Clone> {
    handle: SupervisorHandle,
    pub(crate) heartbeat_failure: Box<dyn Fn(ActorId, Option<B>)>,
    optional_arg: Option<B>,
}

unsafe impl<B: Send + Clone> Send for Supervision<B> {}
unsafe impl<B: Send + Clone> Sync for Supervision<B> {}

impl<B: Send + Clone> Supervision<B> {
    pub fn new(
        handle: SupervisorHandle,
        handler_func: impl Fn(ActorId, Option<B>) + 'static + Send,
        optional_arg: Option<B>,
    ) -> Self {
        Self {
            handle,
            heartbeat_failure: Box::new(handler_func),
            optional_arg,
        }
    }

    async fn heartbeat_check(&self, id: ActorId) -> Result<()> {
        self.handle.heartbeat(id).await
    }
}

impl<B: Send + Clone + 'static> std::future::IntoFuture for Supervision<B> {
    type Output = ();
    type IntoFuture = Pin<Box<dyn std::future::Future<Output = Self::Output> + Send>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let Ok(ids) = self.handle.ids().await else {
                tracing::error!("Error grabbing active IDs, skipping heartbeat check");
                return;
            };

            // for each active ID, perform heartbeat check. If heartbeat fails to register,
            // run the provided recovery function (may be as simple as a log is emitted, or a NOOP)
            for id in ids {
                if let Err(e) = self.heartbeat_check(id).await {
                    tracing::error!(
                        "Heartbeat check failed for actor ID \
                        {id:} {e:}, running provided recovery func"
                    );
                    (self.heartbeat_failure)(id, self.optional_arg.clone());
                } else {
                    tracing::info!("Actor ID {id:} Heartbeat check success");
                }
            }
        })
    }
}
