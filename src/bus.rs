use anymap::{any::Any, Map};
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};

pub const CHANNEL_CAPACITY: usize = 1024;

type AnyMap = Map<dyn Any + Send + Sync>;

#[derive(Clone)]
pub struct MessageBus {
    channels: Arc<Mutex<AnyMap>>,
}

impl MessageBus {
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
