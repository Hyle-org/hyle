//! Event bus used for messaging across components asynchronously.

use crate::utils::static_type_map::Pick;
use anymap::{any::Any, Map};
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};

pub mod command_response;

// Arbitrarily "high enough" value. Memory use is around 200Mb when setting this,
// we can lower it for some rarely used channels if needed.
pub const CHANNEL_CAPACITY: usize = 100000;

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

    async fn receiver<M: BusMessage + Send + Sync + Clone + 'static>(
        &self,
    ) -> broadcast::Receiver<M> {
        self.sender().await.subscribe()
    }

    async fn sender<M: BusMessage + Send + Sync + Clone + 'static>(&self) -> broadcast::Sender<M> {
        self.channels
            .lock()
            .await
            .entry::<broadcast::Sender<M>>()
            .or_insert_with(|| broadcast::channel(CHANNEL_CAPACITY).0)
            .clone()
    }
}

pub mod dont_use_this {
    use super::*;
    /// Get a sender for a specific message type.
    /// Intended for use by BusClient implementations only.
    pub async fn get_sender<M: BusMessage + Send + Sync + Clone + 'static>(
        bus: &SharedMessageBus,
    ) -> broadcast::Sender<M> {
        bus.sender::<M>().await
    }

    pub async fn get_receiver<M: BusMessage + Send + Sync + Clone + 'static>(
        bus: &SharedMessageBus,
    ) -> broadcast::Receiver<M> {
        bus.receiver::<M>().await
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

/// Macro to create  a struct that registers sender/receiver using a shared bus.
/// This can be used to ensure that channels are open without locking in a typesafe manner.
/// It also serves as documentation for the types of messages used by each modules.
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
        #[allow(unused_imports)]
        use $crate::bus::dont_use_this::{get_receiver, get_sender};
        use $crate::utils::static_type_map::static_type_map;
        static_type_map! {
            $(#[$meta])*
            struct $name (
                $(tokio::sync::broadcast::Sender<$sender>,)*
                $(tokio::sync::broadcast::Receiver<$receiver>,)*
            );
        }
        impl $name {
            pub async fn new_from_bus(bus: SharedMessageBus) -> $name {
                $name::new(
                    $(get_sender::<$sender>(&bus).await,)*
                    $(get_receiver::<$receiver>(&bus).await,)*
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
