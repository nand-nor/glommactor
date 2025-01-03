//! A very simple actor framework, built for the glommio runtime

mod actor;
pub mod channel;
mod error;
pub mod handle;
mod supervisor;
use std::future::Future;

pub use actor::{Actor, ActorState, Event, PriorityActor, PriorityEvent, SupervisedActor};
pub use channel::{ChannelRx, ChannelTx, PriorityChannelRx, PriorityRx};
pub use error::ActorError;
pub use handle::{ActorHandle, Handle, PriorityHandle, SupervisedActorHandle, SupervisedHandle};
pub use supervisor::{Supervision, Supervisor, SupervisorHandle, SupervisorMessage};
pub type ActorId = u16;

pub type PriorityActorHandle<T> = handle::ActorHandle<
    T,
    async_priority_channel::Sender<T, PriorityEvent>,
    async_priority_channel::Receiver<T, PriorityEvent>,
>;

use futures::{future::Fuse, FutureExt};

use glommio::{
    executor, spawn_local_into, ExecutorJoinHandle, GlommioError, Latency, LocalExecutorBuilder,
    Placement, Shares,
};

pub fn new_priority_actor_with_handle<T: Event + Send, A: Actor<T> + Sized + Unpin + 'static>(
    constructor: impl FnOnce(async_priority_channel::Receiver<T, PriorityEvent>) -> A,
) -> (A, PriorityActorHandle<T>) {
    let (tx, rx) = async_priority_channel::unbounded();
    handle::ActorHandle::new(constructor, tx, rx)
}

pub struct GlommioSleep {
    pub inner: glommio::timer::Timer,
}

impl futures::Future for GlommioSleep {
    type Output = ();

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        match std::pin::Pin::new(&mut self.inner).poll(cx) {
            std::task::Poll::Ready(_) => std::task::Poll::Ready(()),
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}

unsafe impl Send for GlommioSleep {}
unsafe impl Sync for GlommioSleep {}

impl GlommioSleep {
    pub fn sleep_until(deadline: std::time::Instant) -> impl futures::Future<Output = ()> {
        let duration = deadline.saturating_duration_since(std::time::Instant::now());
        Box::pin(GlommioSleep {
            inner: glommio::timer::Timer::new(duration),
        })
    }

    pub fn sleep(duration: std::time::Duration) -> impl futures::Future<Output = ()> {
        Box::pin(GlommioSleep {
            inner: glommio::timer::Timer::new(duration),
        })
    }
}

/// # Panics
///
/// Will panic if spawning actor onto tq fails
pub fn spawn_exec_actor<T: Event + Send + 'static, A: Actor<T> + 'static>(
    actor: A,
    num_shares: usize,
    latency: Latency,
    placement: Placement,
    name: &'static str,
    tq_name: &'static str,
) -> Result<ExecutorJoinHandle<()>, GlommioError<()>> {
    LocalExecutorBuilder::new(placement)
        .name(name)
        .spawn(move || async move {
            let task = spawn_actor(actor, Shares::Static(num_shares), latency, tq_name)
                .expect("Failure to spawn actor onto tq");
            task.await;
        })
}

/// # Panics
///
/// Will panic if spawning actor onto tq fails
pub fn spawn_exec_actor_with_shutdown<T: Event + Send + 'static, A: Actor<T> + 'static>(
    actor: A,
    num_shares: usize,
    latency: Latency,
    placement: Placement,
    name: &'static str,
    tq_name: &'static str,
    mut receiver: async_broadcast::Receiver<()>,
) -> Result<ExecutorJoinHandle<()>, GlommioError<()>> {
    LocalExecutorBuilder::new(placement)
        .name(name)
        .spawn(move || async move {
            let task = spawn_actor(actor, Shares::Static(num_shares), latency, tq_name)
                .expect("Failure to spawn actor onto tq");
            futures::select! {
                _ = receiver.recv().fuse() => {
                    let msg = format!("Shutdown notice received by {}", name);
                    tracing::info!(msg);
                }
                _ = task.fuse() => {}
            };
        })
}

pub fn spawn_actor<T: Event + Send, A: Actor<T> + 'static>(
    actor: A,
    shares: Shares,
    latency: Latency,
    tq_name: &'static str,
) -> Result<glommio::task::JoinHandle<<A as actor::Actor<T>>::Result>, GlommioError<()>>
where
    <A as Actor<T>>::Result: 'static,
{
    let tq = executor().create_task_queue(shares, latency, tq_name);
    spawn_local_into(Actor::run(actor), tq).map(|t| t.detach())
}

/// # Panics
///
/// Will panic if spawning priority actor fails
pub fn spawn_exec_priority_actor<T: Event + Send + 'static, A: PriorityActor<T> + 'static>(
    actor: A,
    num_shares: usize,
    latency: Latency,
    placement: Placement,
    name: &'static str,
    tq_name: &'static str,
) -> Result<ExecutorJoinHandle<()>, GlommioError<()>> {
    LocalExecutorBuilder::new(placement)
        .name(name)
        .spawn(move || async move {
            let task = spawn_priority_actor(actor, Shares::Static(num_shares), latency, tq_name)
                .expect("Failure to spawn priority actor onto tq");
            task.await;
        })
}

/// # Panics
///
/// Will panic if spawning priority actor fails
pub fn spawn_exec_priority_actor_with_shutdown<
    T: Event + Send + 'static,
    A: PriorityActor<T> + 'static,
>(
    actor: A,
    num_shares: usize,
    latency: Latency,
    placement: Placement,
    name: &'static str,
    tq_name: &'static str,
    mut receiver: async_broadcast::Receiver<()>,
) -> Result<ExecutorJoinHandle<()>, GlommioError<()>> {
    LocalExecutorBuilder::new(placement)
        .name(name)
        .spawn(move || async move {
            let task = spawn_priority_actor(actor, Shares::Static(num_shares), latency, tq_name)
                .expect("Failure to spawn priority actor onto tq");
            futures::select! {
                _ = receiver.recv().fuse() => {
                    let msg = format!("Shutdown notice received by {}", name);
                    tracing::info!(msg);
                }
                _ = task.fuse() => {}
            };
        })
}

pub fn spawn_priority_actor<T: Event + Send, A: PriorityActor<T> + Send + 'static>(
    actor: A,
    shares: Shares,
    latency: Latency,
    tq_name: &'static str,
) -> Result<glommio::task::JoinHandle<<A as actor::Actor<T>>::Result>, GlommioError<()>>
where
    <A as Actor<T>>::Result: 'static,
{
    let tq = executor().create_task_queue(shares, latency, tq_name);
    spawn_local_into(PriorityActor::run(actor), tq).map(|t| t.detach())
}

/// # Panics
///
/// Will panic if spawning handle fut fails
pub fn spawn_exec_handle_fut_with_shutdown(
    num_shares: usize,
    latency: Latency,
    placement: Placement,
    name: &'static str,
    tq_name: &'static str,
    fut: impl Future<Output = ()> + Send + 'static,
    mut receiver: async_broadcast::Receiver<()>,
) -> Result<ExecutorJoinHandle<()>, GlommioError<()>> {
    LocalExecutorBuilder::new(placement)
        .name(name)
        .spawn(move || async move {
            let task = spawn_handle_fut_detached(Shares::Static(num_shares), latency, tq_name, fut)
                .expect("Failure to spawn handle fut onto tq");
            futures::select! {
                _ = receiver.recv().fuse() => {
                    let msg = format!("Shutdown notice received by {}", name);
                    tracing::info!(msg);
                }
                _ = task.fuse() => {}
            };
        })
}

/// # Panics
///
/// Will panic if spawning handle fut fails
pub fn spawn_exec_handle_fut(
    num_shares: usize,
    latency: Latency,
    placement: Placement,
    name: &'static str,
    tq_name: &'static str,
    fut: impl Future<Output = ()> + Send + 'static,
) -> Result<ExecutorJoinHandle<()>, GlommioError<()>> {
    LocalExecutorBuilder::new(placement)
        .name(name)
        .spawn(move || async move {
            let task = spawn_handle_fut_detached(Shares::Static(num_shares), latency, tq_name, fut)
                .expect("Failure to spawn actor onto tq");
            task.await;
        })
}

/// # Panics
///
/// Will panic if spawning handle fut fails
pub fn spawn_exec_handle_futs_with_shutdown<
    F: Future<Output = Vec<()>> + Send + 'static + Unpin,
>(
    placement: Placement,
    name: &'static str,
    mut task_vec: Fuse<F>,
    mut receiver: async_broadcast::Receiver<()>,
) -> Result<ExecutorJoinHandle<()>, GlommioError<()>> {
    LocalExecutorBuilder::new(placement)
        .name(name)
        .spawn(move || async move {
            futures::select! {
                _ = receiver.recv().fuse() => {
                    tracing::info!("Shutdown notice received");
                }
                _ = task_vec => {}
            };
        })
}

/// # Panics
///
/// Will panic if spawning locally into the created task queue fails
pub fn spawn_handle_fut_detached(
    shares: Shares,
    latency: Latency,
    tq_name: &'static str,
    fut: impl Future<Output = ()> + Send + 'static,
) -> Result<glommio::task::JoinHandle<()>, GlommioError<()>> {
    let tq = executor().create_task_queue(shares, latency, tq_name);
    spawn_local_into(fut, tq).map(|t| t.detach()).map_err(|e| {
        tracing::error!("Error spawning actor {e:}");
        e
    })
}

#[cfg(test)]
mod tests {
    use super::handle::ActorHandle;
    use super::*;
    use crate::handle::Handle;

    #[test]
    fn single_event() {
        pub type Reply<T> = flume::Sender<T>;

        pub enum HelloEvent {
            SayHello { reply: Reply<()> },
        }
        impl Event for HelloEvent {}
        struct HelloActor {
            receiver: flume::Receiver<HelloEvent>,
        }

        impl HelloActor {
            fn new(receiver: flume::Receiver<HelloEvent>) -> Self {
                Self { receiver }
            }
            async fn say_hello(&mut self) {
                println!("Hello hi hey!!");
            }

            async fn get_single_event(mut self) -> Result<(), ActorError<HelloEvent>> {
                let HelloEvent::SayHello { reply } = self
                    .receiver
                    .recv_async()
                    .await
                    .expect("Failed to receive event");
                self.say_hello().await;
                reply.send(()).expect("Failed to send reply");

                Ok(())
            }
        }

        #[async_trait::async_trait]
        impl Actor<HelloEvent> for HelloActor
        where
            HelloEvent: Event + Send,
        {
            type Rx = futures::channel::mpsc::Receiver<HelloEvent>;
            type Error = ActorError<HelloEvent>;
            type Result = Result<(), Self::Error>;
            async fn run(self) -> Self::Result {
                self.get_single_event().await
            }
        }

        impl ActorHandle<HelloEvent, flume::Sender<HelloEvent>, flume::Receiver<HelloEvent>> {
            async fn say_hello(&self) -> Result<(), ActorError<HelloEvent>> {
                let (tx, rx) = flume::bounded(1);
                let msg = HelloEvent::SayHello { reply: tx };
                self.send(msg)
                    .await
                    .expect("Faied to send from actor handle");
                rx.recv_async()
                    .await
                    .expect("Failed to recv from actor handle");
                Ok(())
            }
        }

        let (sender, receiver) = flume::unbounded();

        let (actor, handle) = ActorHandle::new(HelloActor::new, sender, receiver);

        let handle = glommio::LocalExecutorBuilder::new(glommio::Placement::Fixed(0))
            .name(&format!("{}{}", "test-handle", 0))
            .spawn(move || async move {
                let tq = glommio::executor().create_task_queue(
                    glommio::Shares::default(),
                    glommio::Latency::NotImportant,
                    "test",
                );

                let task = glommio::spawn_local_into(actor.run(), tq)
                    .map(|t| t.detach())
                    .expect("Failed to spawn detached task");
                handle.say_hello().await.expect("Failed to say hello");
                task.await;
            })
            .expect("Failed to execute");

        handle.join().expect("Failed to join glommio join handle");
    }
}
