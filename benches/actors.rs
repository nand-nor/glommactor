//! Benchmarks comparing act_zero and glommactor
#[macro_use]
extern crate bencher;

use act_zero::runtimes::glommio::spawn_actor_with_tq;
use act_zero::*;
use bencher::Bencher;
use glommactor::{
    handle::{ActorHandle, Handle},
    spawn_exec_actor, spawn_exec_handle_fut, Actor, ActorError, Event,
};
use glommio::{executor, Latency, LocalExecutorBuilder, Placement, Shares};
use std::time::Duration;

pub type Reply<T> = flume::Sender<T>;

#[derive(Clone, Debug)]
pub enum HelloWorldEvent {
    SayHello { reply: Reply<()> },
    State { reply: Reply<HelloState> },
}

#[derive(Clone, Debug)]
pub enum HelloState {
    Stopped,
    Started,
    Running,
}

impl Event for HelloWorldEvent {}

struct HelloWorldActor {
    receiver: flume::Receiver<HelloWorldEvent>,
    state: HelloState,
}

impl HelloWorldActor {
    fn new(receiver: flume::Receiver<HelloWorldEvent>) -> Self {
        Self {
            receiver,
            state: HelloState::Started,
        }
    }
}

// impls needed for act_zero
impl act_zero::Actor for HelloWorldActor {}

impl act_zero::IntoActorResult for HelloState {
    type Output = HelloState;

    fn into_actor_result(self) -> ActorResult<Self::Output> {
        Ok(Produces::Value(self))
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
    type State = HelloState;
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

    async fn state(&mut self) -> HelloState {
        self.state.clone()
    }

    async fn event_loop(mut self) -> Result<(), ActorError<HelloWorldEvent>> {
        self.state = HelloState::Running;
        while let Ok(event) = self.receiver.recv_async().await {
            self.process(event).await
        }
        Ok(())
    }

    async fn process(&mut self, event: HelloWorldEvent) {
        match event {
            HelloWorldEvent::SayHello { reply } => {
                {
                    self.say_hello().await;
                    reply.send(())
                }
                .ok();
            }
            HelloWorldEvent::State { reply } => {
                reply.send(self.state().await).ok();
            }
        }
    }
}

fn glommactor_main() -> Result<(), ActorError<HelloWorldEvent>> {
    let mut handle_vec = vec![];

    let (tx, rx) = flume::unbounded();
    // create actor and handle before running in local executor tasks
    let (actor, handle) = ActorHandle::new(HelloWorldActor::new, tx, rx);
    let handle_wrapper = HandleWrapper { handle };

    // pin actor to core 0
    handle_vec.push(
        spawn_exec_actor(
            actor,
            10,
            Latency::Matters(Duration::from_millis(1)),
            Placement::Fixed(3),
            "rt-actor",
            "tq-actor",
        )
        .expect("Unable to spawn actor onto runtime"),
    );

    // define a future for the handle spawner function to execute
    let fut = async move {
        handle_wrapper.say_hello().await.ok();
        let _state = handle_wrapper.state().await.expect("Failed to get state");
    };

    // pins future where handle to actor is operating to core 1
    handle_vec.push(
        spawn_exec_handle_fut(
            10,
            Latency::Matters(Duration::from_millis(10)),
            Placement::Fixed(0),
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

fn act_zero_main() -> Result<(), act_zero::ActorError> {
    let mut handle_vec = vec![];

    handle_vec.push(
        LocalExecutorBuilder::new(Placement::Fixed(2))
            .name(&format!("{}{}", "actor", 0))
            .spawn(move || async move {
                let tq: glommio::TaskQueueHandle = executor().create_task_queue(
                    Shares::Static(10),
                    Latency::Matters(std::time::Duration::from_millis(10)),
                    "other-actor-tq",
                );

                let (_tx, rx) = flume::unbounded();
                let addr = spawn_actor_with_tq(
                    HelloWorldActor {
                        receiver: rx,
                        state: HelloState::Stopped,
                    },
                    tq,
                );
                call!(addr.say_hello()).await.unwrap();
                let _state = call!(addr.state()).await.unwrap();

                /*  Add some other work to the task queue
                let (addr_tx, addr_rx) = flume::unbounded();
                addr_tx.send_async(addr.clone()).await.expect("Failed");

                // add some other work to the task queue
                let fut = async move {
                    let fut_addr = addr_rx.recv_async().await.expect("Failed");
                    call!(fut_addr.say_hello()).await.unwrap();
                    let _state =  call!(fut_addr.state()).await.unwrap();
                };

                let t = glommio::spawn_local_into(fut, tq).map(|t| t.detach()).expect("Failed");
                t.await;
                */
            })
            .unwrap(),
    );

    handle_vec.push(
        LocalExecutorBuilder::new(Placement::Fixed(3))
            .name(&format!("{}{}", "busy-work", 1))
            .spawn(move || async move {
                // busy work to simulate action on 2 cores
                for i in 0..1000 {
                    let _x = i + 1;
                }
            })
            .unwrap(),
    );

    for handle in handle_vec {
        handle.join().unwrap();
    }

    Ok(())
}

fn test_glom(bencher: &mut Bencher) {
    bencher.iter(|| glommactor_main().expect("Failure to bench using glommactor_main"));
}

fn test_actzero(bencher: &mut Bencher) {
    bencher.iter(|| act_zero_main().expect("Failure to bench using act_zero_main"));
}

benchmark_group!(test_glommactor, test_glom);

benchmark_group!(test_act_zero, test_actzero);

benchmark_main!(test_glommactor, test_act_zero);
