use crate::{Event, PriorityEvent};

#[async_trait::async_trait]
pub trait ChannelTx<E: Event + Send> {
    type Result;
    type Error;
    async fn send_event(&self, s: E) -> Self::Result;
}
pub trait ChannelRx<E: Event + Send> {}

pub trait Channel {}

pub type PriorityRx<E> = async_priority_channel::Receiver<E, PriorityEvent>;
pub type PriorityTx<E> = async_priority_channel::Sender<E, PriorityEvent>;

#[async_trait::async_trait]
pub trait PriorityChannelTx<E: Event + Send>: ChannelTx<E> {
    type Result;
    type Error;
    async fn send_with_priority(
        &self,
        s: E,
        priority: PriorityEvent,
    ) -> <Self as ChannelTx<E>>::Result;
}
pub trait PriorityChannelRx<E: Event + Send>: ChannelRx<E> {}

impl<E: Event + Send> PriorityChannelRx<E> for PriorityRx<E> {}

#[async_trait::async_trait]
impl<E: Event + Send> ChannelTx<E> for flume::Sender<E> {
    type Result = Result<(), Self::Error>;
    type Error = flume::SendError<E>;
    async fn send_event(&self, s: E) -> Self::Result {
        self.send_async(s).await
    }
}

impl<E: Event + Send> ChannelRx<E> for flume::Receiver<E> {}

#[async_trait::async_trait]
impl<E: Event + Send> ChannelTx<E> for async_priority_channel::Sender<E, PriorityEvent> {
    type Result = Result<(), Self::Error>;
    type Error = async_priority_channel::SendError<(E, PriorityEvent)>;
    async fn send_event(&self, s: E) -> Self::Result {
        self.send(s, PriorityEvent::default()).await
    }
}

#[async_trait::async_trait]
impl<E: Event + Send> PriorityChannelTx<E> for async_priority_channel::Sender<E, PriorityEvent> {
    type Result = Result<(), <Self as PriorityChannelTx<E>>::Error>;
    type Error = async_priority_channel::SendError<(E, PriorityEvent)>;
    async fn send_with_priority(
        &self,
        s: E,
        priority: PriorityEvent,
    ) -> <Self as ChannelTx<E>>::Result {
        self.send(s, priority).await
    }
}

impl<E: Event + Send> ChannelRx<E> for async_priority_channel::Receiver<E, PriorityEvent> {}
