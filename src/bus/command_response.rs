use std::sync::Arc;
use std::time::Duration;

use super::BusMessage;
use anyhow::bail;
use anyhow::Result;
use tokio::sync::Mutex;

use crate::bus::BusClientSender;

pub const CLIENT_TIMEOUT_SECONDS: u64 = 10;

#[derive(Clone, Debug)]
pub struct Query<Type, Answer>(Arc<Mutex<Option<InnerQuery<Type, Answer>>>>);
impl<Type, Answer> Query<Type, Answer> {
    pub fn take(self) -> Result<InnerQuery<Type, Answer>> {
        match self.0.try_lock() {
            Ok(mut guard) => match guard.take() {
                Some(inner) => Ok(inner),
                None => bail!("Query already answered"),
            },
            Err(_) => bail!("Query already answered"),
        }
    }
}

#[derive(Debug)]
pub struct InnerQuery<Type, Answer> {
    pub callback: tokio::sync::oneshot::Sender<Result<Answer>>,
    pub data: Type,
}
impl<Cmd, Res> BusMessage for Query<Cmd, Res> {}
impl<Cmd, Res> InnerQuery<Cmd, Res> {
    pub fn answer(self, data: Res) -> Result<()> {
        self.callback
            .send(Ok(data))
            .map_err(|_| anyhow::anyhow!("Error while sending response"))
    }
    pub fn bail<T>(self, error: T) -> Result<()>
    where
        T: Into<anyhow::Error>,
    {
        self.callback
            .send(Err(error.into()))
            .map_err(|_| anyhow::anyhow!("Error while sending response"))
    }
}

pub trait CmdRespClient<Cmd, Res>
where
    Cmd: Clone + Send + Sync + 'static,
    Res: Clone + Send + Sync + 'static,
{
    fn request(&mut self, cmd: Cmd) -> impl std::future::Future<Output = Result<Res>> + Send;
}

impl<Cmd, Res, T: BusClientSender<Query<Cmd, Res>> + Send> CmdRespClient<Cmd, Res> for T
where
    Cmd: Clone + Send + Sync + 'static,
    Res: Clone + Send + Sync + 'static,
{
    async fn request(&mut self, cmd: Cmd) -> Result<Res> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let query_cmd = Query(Arc::new(Mutex::new(Some(InnerQuery {
            callback: tx,
            data: cmd,
        }))));

        _ = self.send(query_cmd);

        match tokio::time::timeout(Duration::from_secs(CLIENT_TIMEOUT_SECONDS), rx).await {
            Ok(Ok(res)) => res,
            Ok(Err(e)) => bail!("Error while calling topic: {}", e),
            Err(timeouterror) => bail!(
                "Timeout triggered while calling topic with query: {}",
                timeouterror.to_string()
            ),
        }
    }
}

pub mod handle_messages_helpers {
    use crate::bus::metrics::BusMetrics;
    use crate::utils::static_type_map::Pick;
    pub fn receive_bus_metrics<Msg: 'static, Client: Pick<BusMetrics> + 'static>(
        _bus: &mut Client,
    ) {
        Pick::<BusMetrics>::get_mut(_bus).receive::<Msg, Client>();
    }
}

#[macro_export]
macro_rules! handle_messages {
    (on_bus $bus:expr, $($rest:tt)*) => {

        handle_messages! {
            bus($bus) $($rest)*
        }
    };

    (bus($bus:expr) command_response<$command:ty, $response:ty> $res:pat => $handler:block $($rest:tt)*) => {{
        use paste::paste;
        use $crate::bus::command_response::*;
        #[allow(unused_imports)]
        use $crate::utils::static_type_map::Pick;
        #[allow(unused_imports)]
        use $crate::bus::command_response::handle_messages_helpers::receive_bus_metrics;
        paste! {
            handle_messages! {
                bus($bus) $($rest)*
                Ok(_raw_query) = #[allow(clippy::macro_metavars_in_unsafe)] unsafe { &mut *Pick::<tokio::sync::broadcast::Receiver<Query<$command, $response>>>::splitting_get_mut(&mut $bus) }.recv() => {
                    receive_bus_metrics::<Query<$command, $response>,_>(&mut $bus);
                    if let Ok(mut _value) = _raw_query.take() {
                        let $res = &mut _value.data;
                        let res: Result<$response> = $handler;
                        match res {
                            Ok(res) => {
                                if let Err(e) = _value.answer(res) {
                                    tracing::error!("Error while answering query: {}", e);
                                }
                            }
                            Err(e) => {
                                if let Err(e) = _value.bail(e) {
                                    tracing::error!("Error while answering query: {}", e);
                                }
                            }
                        }
                    } else {
                        tracing::error!("Query already answered");
                    }
                }
            }
        }
    }};

    (bus($bus:expr) listen<$message:ty> $res:pat => $handler:block $($rest:tt)*) => {{
        use paste::paste;
        #[allow(unused_imports)]
        use $crate::utils::static_type_map::Pick;
        #[allow(unused_imports)]
        use $crate::bus::command_response::handle_messages_helpers::receive_bus_metrics;
        paste! {
            handle_messages! {
                bus($bus) $($rest)*
                Ok($res) = unsafe { &mut *Pick::<tokio::sync::broadcast::Receiver<$message>>::splitting_get_mut(&mut $bus) }.recv()  => {
                    receive_bus_metrics::<$message, _>(&mut $bus);
                    $handler
                }
            }
        }
    }};

    // Fallback to normal select cases
    (bus($bus:expr) $($rest:tt)+) => {
        loop {
            tokio::select! {
                $($rest)+
            }
        }
    };
}

pub(crate) use handle_messages;

#[cfg(test)]
mod test {
    use super::*;
    use crate::bus::{bus_client, SharedMessageBus};

    bus_client!(
        struct TestBusClient {
            sender(Query<i32, u8>),
            receiver(Query<i32, u8>),
        }
    );

    #[tokio::test]
    async fn test_cmd_resp() {
        let shared_bus = SharedMessageBus::default();
        let mut sender = TestBusClient::new_from_bus(shared_bus.new_handle()).await;
        let mut receiver = TestBusClient::new_from_bus(shared_bus).await;

        // Spawn a task to handle the query
        tokio::spawn(async move {
            handle_messages! {
                on_bus receiver,
                command_response<i32, u8> _ => {
                    Ok(3)
                }
            }
        });
        let res = sender.request(42);

        assert_eq!(res.await.unwrap(), 3);
    }
}
