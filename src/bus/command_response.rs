use std::sync::Arc;
use std::time::Duration;

use super::BusMessage;
use anyhow::bail;
use anyhow::Result;
use tokio::sync::Mutex;

use crate::bus::BusClientSender;

pub const CLIENT_TIMEOUT_SECONDS: u64 = 10;

pub trait NeedAnswer<Answer>: BusMessage {}

#[derive(Clone)]
pub struct Query<Type, Answer> {
    pub callback: Arc<Mutex<Option<tokio::sync::oneshot::Sender<Result<Answer>>>>>,
    pub data: Type,
}
impl<Cmd, Res> BusMessage for Query<Cmd, Res> {}

pub trait CmdRespClient<Cmd, Res>
where
    Cmd: NeedAnswer<Res> + Clone + Send + Sync + 'static,
    Res: Clone + Send + Sync + 'static,
{
    fn request(&mut self, cmd: Cmd) -> impl std::future::Future<Output = Result<Res>> + Send;
}

impl<Cmd, Res, T: BusClientSender<Query<Cmd, Res>> + Send> CmdRespClient<Cmd, Res> for T
where
    Cmd: NeedAnswer<Res> + Clone + Send + Sync + 'static,
    Res: Clone + Send + Sync + 'static,
{
    async fn request(&mut self, cmd: Cmd) -> Result<Res> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let query_cmd = Query {
            callback: Arc::new(Mutex::new(Some(tx))),
            data: cmd,
        };

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

#[macro_export]
macro_rules! handle_messages {
    (on_bus $bus:expr, $($rest:tt)*) => {
        handle_messages! {
            bus($bus) $($rest)*
        }
    };

    (bus($bus:expr) command_response<$command:ident, $response:ty> $res:pat => $handler:block $($rest:tt)*) => {{
        use paste::paste;
        use $crate::bus::command_response::*;
        #[allow(unused_imports)]
        use $crate::utils::generic_tuple::Pick;
        paste! {
            handle_messages! {
                bus($bus) $($rest)*
                Ok(_value) = unsafe { &mut *Pick::<tokio::sync::broadcast::Receiver<Query<$command, $response>>>::splitting_get_mut(&mut $bus) }.recv() => {
                    let $res = _value.data;
                    let res = $handler;
                    if let Some(cb) = _value.callback.lock().await.take() {
                        cb.send(res).unwrap();
                    }
                }
            }
        }
    }};

    (bus($bus:expr) listen<$message:ty> $res:pat => $handler:block $($rest:tt)*) => {{
        use paste::paste;
        #[allow(unused_imports)]
        use $crate::utils::generic_tuple::Pick;
        paste! {
            handle_messages! {
                bus($bus) $($rest)*
                Ok($res) = unsafe { &mut *Pick::<tokio::sync::broadcast::Receiver<$message>>::splitting_get_mut(&mut $bus) }.recv()  => {
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
