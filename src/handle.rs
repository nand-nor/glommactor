use std::marker::PhantomData;

use crate::{
    channel::{ChannelTx, PriorityChannelTx},
    Actor, ActorError, ActorId, ActorState, ChannelRx, Event, PriorityChannelRx, PriorityEvent,
    SupervisorMessage,
};

#[async_trait::async_trait]
pub trait Handle<T: Event + Send> {
    type State;
    type Result;
    async fn send(&self, event: T) -> Self::Result;
}

#[async_trait::async_trait]
impl<T: Event + Send + Sync, S: ChannelTx<T> + Send + Sync, R: ChannelRx<T> + Send + Sync> Handle<T>
    for ActorHandle<T, S, R>
{
    type State = Box<dyn State>;
    type Result = <S as ChannelTx<T>>::Result;
    async fn send(&self, event: T) -> Self::Result {
        self.sender.send_event(event).await
    }
}

#[async_trait::async_trait]
pub trait PriorityHandle<T: Event + Send> {
    type State;
    type Result;
    async fn send_with_priority(&self, event: T, priority: PriorityEvent) -> Self::Result;
}

pub trait State {}

#[async_trait::async_trait]
impl<
        T: Event + Send + Sync,
        S: PriorityChannelTx<T> + Send + Sync,
        R: PriorityChannelRx<T> + Send + Sync,
    > PriorityHandle<T> for ActorHandle<T, S, R>
{
    type State = Box<dyn State>;
    type Result = <S as ChannelTx<T>>::Result;
    async fn send_with_priority(&self, event: T, priority: PriorityEvent) -> Self::Result {
        self.sender.send_with_priority(event, priority).await
    }
}

#[derive(Clone)]
pub struct ActorHandle<T: Event + Send, S: ChannelTx<T>, R: ChannelRx<T>> {
    sender: S,
    phantom: PhantomData<(T, R)>,
}

impl<T: Event + Send, S: ChannelTx<T>, R: ChannelRx<T>> ActorHandle<T, S, R> {
    pub fn new<A: Actor<T> + Sized + Unpin + 'static>(
        constructor: impl FnOnce(R) -> A,
        sender: S,
        receiver: R,
    ) -> (A, Self) {
        let actor = constructor(receiver);
        (
            actor,
            Self {
                sender,
                phantom: PhantomData,
            },
        )
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

#[async_trait::async_trait]
impl<T: Event + Send> Handle<T> for SupervisedActorHandle<T> {
    type State = ActorState;
    type Result = std::result::Result<(), flume::SendError<T>>;
    async fn send(&self, event: T) -> Self::Result {
        self.sender.send_event(event).await
    }
}

#[async_trait::async_trait]
pub trait SupervisedHandle<T: Event + Send>: Handle<T> {
    type Rx;
    type Error;
    async fn send_shutdown(&self) -> Result<(), Self::Error>;
    async fn send_start(&self) -> Result<(), Self::Error>;
    async fn subscribe_direct(&self) -> Self::Rx;
    async fn actor_state(&self) -> Result<<Self as Handle<T>>::State, Self::Error>;
}

#[async_trait::async_trait]
impl<T: Event + Send> SupervisedHandle<T> for SupervisedActorHandle<T> {
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
    async fn actor_state(&self) -> Result<<Self as Handle<T>>::State, Self::Error> {
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
