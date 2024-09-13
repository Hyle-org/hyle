use anymap::{any::Any, Map};
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};

pub const CHANNEL_CAPACITY: usize = 1024;

type AnyMap = Map<dyn Any + Send + Sync>;

pub struct SharedMessageBus {
    channels: Arc<Mutex<AnyMap>>,
}

impl SharedMessageBus {
    pub fn new_handle(&self) -> Self {
        SharedMessageBus {
            channels: Arc::clone(&self.channels),
        }
    }

    pub fn new() -> Self {
        Self {
            channels: Arc::new(Mutex::new(AnyMap::new())),
        }
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
