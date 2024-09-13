use anymap::{any::Any, Map};
use std::sync::{atomic::AtomicUsize, Arc};
use tokio::sync::{broadcast, Mutex};

pub mod command_response;

pub const CHANNEL_CAPACITY: usize = 1024;

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
