//! Event bus used for messaging across components asynchronously.

use crate::utils::static_type_map::Pick;
use anymap::{any::Any, Map};
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};

pub mod command_response;

pub const CHANNEL_CAPACITY: usize = 1000000;

type AnyMap = Map<dyn Any + Send + Sync>;

/// Types that implement BusMessage can be sent on the bus - this is mostly for documentation purposes.
pub trait BusMessage {}

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

    pub async fn receiver<M: BusMessage + Send + Sync + Clone + 'static>(
        &self,
    ) -> broadcast::Receiver<M> {
        self.sender().await.subscribe()
    }

    pub async fn sender<M: BusMessage + Send + Sync + Clone + 'static>(
        &self,
    ) -> broadcast::Sender<M> {
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

pub trait BusClientSender<T> {
    fn send(&self, message: T) -> Result<usize, tokio::sync::broadcast::error::SendError<T>>;
}
pub trait BusClientReceiver<T> {
    fn recv(
        &mut self,
    ) -> impl std::future::Future<Output = Result<T, tokio::sync::broadcast::error::RecvError>> + Send;
}

macro_rules! bus_client {
    (
        $(#[$meta:meta])*
        struct $name:ident {
            $(sender($sender:ty),)*
            $(receiver($receiver:ty),)*
        }
    ) => {
        #[allow(unused_imports)]
        use $crate::bus::BusClientReceiver;
        #[allow(unused_imports)]
        use $crate::bus::BusClientSender;
        use $crate::utils::static_type_map::static_type_map;

        $(#[$meta])*
        static_type_map! {
            struct $name (
                $(tokio::sync::broadcast::Sender<$sender>,)*
                $(tokio::sync::broadcast::Receiver<$receiver>,)*
            );
        }
        impl $name {
            pub async fn new_from_bus(bus: SharedMessageBus) -> $name {
                $name::new(
                    $(bus.sender::<$sender>().await,)*
                    $(bus.receiver::<$receiver>().await,)*
                )
            }
        }
    };
}
pub(crate) use bus_client;

impl<T, M: Clone> BusClientSender<M> for T
where
    T: Pick<tokio::sync::broadcast::Sender<M>>,
{
    fn send(&self, message: M) -> Result<usize, tokio::sync::broadcast::error::SendError<M>> {
        self.get().send(message)
    }
}

impl<T, M: 'static + Clone + Send> BusClientReceiver<M> for T
where
    T: Pick<tokio::sync::broadcast::Receiver<M>>,
{
    fn recv(
        &mut self,
    ) -> impl std::future::Future<Output = Result<M, tokio::sync::broadcast::error::RecvError>> + Send
    {
        self.get_mut().recv()
    }
}
