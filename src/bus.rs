//! Event bus used for messaging across components asynchronously.

use anymap::{any::Any, Map};
use frunk::{hlist::Selector, HCons};
use std::{
    future::Future,
    sync::{atomic::AtomicUsize, Arc},
};
use tokio::sync::{
    broadcast::{self, error::RecvError, Receiver, Sender},
    Mutex,
};

pub mod command_response;

pub const CHANNEL_CAPACITY: usize = 1000000;

type AnyMap = Map<dyn Any + Send + Sync>;

pub struct SharedMessageBus {
    ids: Arc<Mutex<AnyMap>>,
    channels: Arc<Mutex<AnyMap>>,
}

impl SharedMessageBus {
    pub fn new_handle(&self) -> Self {
        SharedMessageBus {
            ids: Arc::clone(&self.ids),
            channels: Arc::clone(&self.channels),
        }
    }

    pub fn new() -> Self {
        Self {
            ids: Arc::new(Mutex::new(AnyMap::new())),
            channels: Arc::new(Mutex::new(AnyMap::new())),
        }
    }

    //TODO: manage locks at entry level (whole map locks to generate the next id of one kind of channel)
    pub async fn next_id<M: Send + Sync + Clone + 'static>(&self) -> usize {
        self.ids
            .lock()
            .await
            .entry::<(Option<M>, AtomicUsize)>()
            .or_insert_with(|| (None, AtomicUsize::new(0)))
            .1
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    pub async fn receiver<M: Send + Sync + Clone + 'static>(&self) -> broadcast::Receiver<M> {
        self.sender().await.subscribe()
    }

    pub async fn sender<M: Send + Sync + Clone + 'static>(&self) -> broadcast::Sender<M> {
        self.channels
            .lock()
            .await
            .entry::<broadcast::Sender<M>>()
            .or_insert_with(|| broadcast::channel(CHANNEL_CAPACITY).0)
            .clone()
    }
}

impl Default for SharedMessageBus {
    fn default() -> Self {
        Self::new()
    }
}

pub struct BusClient<T> {
    inner: T,
}

impl<T> BusClient<T> {
    pub fn new() -> BusClient<T>
    where
        T: Default,
    {
        BusClient::<T> {
            inner: T::default(),
        }
    }

    pub async fn with_sender<M: Send + Sync + Clone + 'static>(
        self,
        bus: &SharedMessageBus,
    ) -> BusClient<HCons<Sender<M>, T>> {
        let sender = bus.sender::<M>().await;
        BusClient {
            inner: HCons {
                head: sender,
                tail: self.inner,
            },
        }
    }
    pub async fn with_receiver<M: Send + Sync + Clone + 'static>(
        self,
        bus: &SharedMessageBus,
    ) -> BusClient<HCons<Receiver<M>, T>> {
        let sender = bus.receiver::<M>().await;
        BusClient {
            inner: HCons {
                head: sender,
                tail: self.inner,
            },
        }
    }
    pub fn send<M: Send + Sync + Clone + 'static, U>(
        &self,
        message: M,
    ) -> Result<usize, broadcast::error::SendError<M>>
    where
        T: Selector<Sender<M>, U>,
    {
        let sender = self.inner.get();
        sender.send(message)
    }

    pub fn recv<M: Send + Sync + Clone + 'static, U>(
        &mut self,
    ) -> impl Future<Output = Result<M, RecvError>> + '_
    where
        T: Selector<Receiver<M>, U>,
    {
        let receiver = self.inner.get_mut();
        receiver.recv()
    }
}
