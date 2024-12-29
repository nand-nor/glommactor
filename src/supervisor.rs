use std::collections::{HashMap, HashSet};

use crate::{handle::ActorHandle, Actor, ActorError, ActorId, ActorState, Event};

pub type Reply<T> = flume::Sender<T>;
pub type SupervisorActionReply<T> = Reply<Result<T>>;
pub type Result<T> = std::result::Result<T, ActorError<SupervisorAction>>;

#[derive(Clone, Debug)]
pub enum SupervisorMessage {
    SetId { id: ActorId },
    Id { reply: Reply<ActorId> },
    Start,
    Stop,
    ClearAll,
    Shutdown,
    Backup,
    State { reply: Reply<ActorState> },
}

pub struct Supervisor {
    /// Stores sender end for SupervisorMessages by Actor ID
    bcast: HashMap<ActorId, flume::Sender<SupervisorMessage>>,
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
            bcast: HashMap::default(),
            ids,
            receiver,
        }
    }

    async fn event_loop(mut self) -> Result<()> {
        loop {
            match self.receiver.recv_async().await {
                Ok(event) => self.process(event).await,
                Err(e) => {
                    tracing::warn!("Channel error {e:}");
                    break;
                }
            }
        }
        Ok(())
    }

    async fn process(&mut self, action: SupervisorAction) {
        tracing::trace!("Processing supervisor action {action:?}");
        match action {
            SupervisorAction::NewActorSetup { reply } => {
                reply.send(self.new_supervisee_setup()).ok();
            }
            SupervisorAction::ShutdownAll { reply } => {
                let keys = self.bcast.keys().copied().collect::<Vec<_>>();
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
            SupervisorAction::ActorState { id, reply } => {
                reply.send(self.actor_state(id).await).ok();
            }
            SupervisorAction::GetCurrentIds { reply } => {
                reply.send(self.get_current_ids().await).ok();
            }
        }
        tracing::debug!("Processed");
    }

    pub fn new_supervisee_setup(
        &mut self,
    ) -> Result<(
        flume::Sender<SupervisorMessage>,
        flume::Receiver<SupervisorMessage>,
        ActorId,
    )> {
        let (tx, rx) = flume::unbounded();
        // todo this could in theory run out of IDs, should check the set is not empty
        let id = *self
            .ids
            .iter()
            .next()
            .ok_or_else(|| ActorError::SystemError("too many actors".to_string()))?;

        self.ids.take(&id);
        let _ = tx.send(SupervisorMessage::SetId { id });
        self.bcast.insert(id, tx.clone());
        Ok((tx, rx, id))
    }

    pub async fn shutdown_actor(&mut self, id: ActorId) -> Result<()> {
        tracing::trace!("Supervisor shutting down actor id {id:}");
        if let Some(sender) = self.bcast.get(&id) {
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

    async fn actor_state(&mut self, id: ActorId) -> Result<ActorState> {
        tracing::trace!("Supervisor shutting down actor id {id:}");
        if let Some(sender) = self.bcast.get(&id) {
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

    // to be used in methods that monitor current state of all
    // actors, in order to restart if needed. TODO: configure
    // heartbeat for actors reporting to supervisor
    async fn get_current_ids(&mut self) -> Vec<ActorId> {
        self.bcast.keys().copied().collect::<Vec<_>>()
    }
}

#[derive(Clone, Debug)]
pub enum SupervisorAction {
    ShutdownAll {
        reply: SupervisorActionReply<()>,
    },
    Shutdown {
        id: ActorId,
        reply: SupervisorActionReply<()>,
    },
    NewActorSetup {
        reply: SupervisorActionReply<(
            flume::Sender<SupervisorMessage>,
            flume::Receiver<SupervisorMessage>,
            ActorId,
        )>,
    },
    GetCurrentIds {
        reply: Reply<Vec<ActorId>>,
    },
    ActorState {
        id: ActorId,
        reply: SupervisorActionReply<ActorState>,
    },
}

/// Supervisor can be a stand-alone entity or
/// can be created and run like an un-supervised
/// actor itself
#[derive(Clone)]
pub struct SupervisorHandle {
    handle: ActorHandle<SupervisorAction>,
}

impl Event for SupervisorAction {}

impl SupervisorHandle {
    pub fn new() -> (Supervisor, Self) {
        // create supervisor
        let (supervisor, handle) = ActorHandle::new(Supervisor::new);

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
        let msg = SupervisorAction::ActorState { id, reply: tx };
        self.handle.send(msg).await?;

        rx.recv_async().await.map_err(|e| {
            let msg = format!("Send error {e:?}");
            tracing::error!(msg);
            ActorError::ActorError(msg)
        })?
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
