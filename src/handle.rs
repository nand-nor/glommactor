use crate::{Actor, ActorError, ActorId, ActorState, Event, SupervisorMessage};

pub trait Handle {
    type State;
}

pub trait State {}

impl<T: Event + Send> Handle for ActorHandle<T> {
    type State = Box<dyn State>;
}

#[derive(Clone)]
pub struct ActorHandle<T: Event + Send> {
    sender: flume::Sender<T>,
}

impl<T: Event + Send> ActorHandle<T> {
    pub fn new<A: Actor<T> + Sized + Unpin + 'static>(
        constructor: impl FnOnce(flume::Receiver<T>) -> A,
    ) -> (A, Self) {
        let (sender, receiver) = flume::unbounded();
        let actor = constructor(receiver);

        (actor, Self { sender })
    }

    pub async fn send(&self, event: T) -> Result<(), ActorError<T>> {
        self.sender
            .send_async(event)
            .await
            .map_err(ActorError::from)
    }
}

pub struct SupervisedActorHandle<T: Event + Send> {
    sender: flume::Sender<T>,
    broadcast: flume::Sender<SupervisorMessage>,
    id: ActorId,
}

impl<T: Event + Send> Clone for SupervisedActorHandle<T> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            broadcast: self.broadcast.clone(),
            id: self.id,
        }
    }
}

impl<T: Event + Send> Handle for SupervisedActorHandle<T> {
    type State = ActorState;
}

#[async_trait::async_trait]
pub trait SupervisedHandle: Handle {
    type Rx;
    type Error;
    async fn send_shutdown(&self) -> Result<(), Self::Error>;
    async fn send_start(&self) -> Result<(), Self::Error>;
    async fn subscribe_direct(&self) -> Self::Rx;
    async fn actor_state(&self) -> Result<<Self as Handle>::State, Self::Error>;
}

#[async_trait::async_trait]
impl<T: Event + Send> SupervisedHandle for SupervisedActorHandle<T> {
    type Rx = flume::Sender<T>;
    type Error = ActorError<SupervisorMessage>;
    async fn send_shutdown(&self) -> Result<(), Self::Error> {
        self.supervisor_send(SupervisorMessage::Shutdown).await
    }
    async fn send_start(&self) -> Result<(), Self::Error> {
        self.supervisor_send(SupervisorMessage::Start).await
    }
    async fn subscribe_direct(&self) -> Self::Rx {
        self.handle_subscribe_direct().await
    }
    async fn actor_state(&self) -> Result<<Self as Handle>::State, Self::Error> {
        self.get_state().await
    }
}

impl<T: Event + Send> SupervisedActorHandle<T> {
    pub fn new<A: Actor<T>>(
        broadcast: flume::Sender<SupervisorMessage>,
        bcast_rx: flume::Receiver<SupervisorMessage>,
        id: ActorId,
        constructor: impl FnOnce(flume::Receiver<T>, flume::Receiver<SupervisorMessage>) -> A,
    ) -> (A, Self) {
        let (sender, receiver) = flume::unbounded();
        let actor = constructor(receiver, bcast_rx);
        (
            actor,
            Self {
                id,
                sender,
                broadcast,
            },
        )
    }

    pub async fn handle_subscribe_direct(&self) -> flume::Sender<T> {
        self.sender.clone()
    }

    pub async fn handle_subscribe_bcast(&self) -> flume::Sender<SupervisorMessage> {
        self.broadcast.clone()
    }

    pub async fn get_state(&self) -> Result<ActorState, ActorError<SupervisorMessage>> {
        let (tx, rx) = flume::bounded(1);
        let msg = SupervisorMessage::State { reply: tx };
        self.broadcast.send_async(msg).await?;

        rx.recv_async().await.map_err(|e| {
            let msg = format!("Send cancelled {e:}");
            tracing::error!(msg);
            ActorError::ActorError(msg)
        })
    }

    pub async fn send(&self, event: T) -> Result<(), ActorError<T>> {
        self.sender
            .send_async(event)
            .await
            .map_err(ActorError::from)
    }

    pub(crate) async fn supervisor_send(
        &self,
        msg: SupervisorMessage,
    ) -> Result<(), ActorError<SupervisorMessage>> {
        self.broadcast
            .send_async(msg)
            .await
            .map_err(ActorError::from)
    }
}
