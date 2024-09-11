use super::SharedMessageBus;
use anyhow::Result;

pub trait Listener<Cmd> {
    async fn spawn_listen(&self, routes: fn(Cmd)) -> &Self;
}

impl<Cmd: Clone + Send + Sync + 'static> Listener<Cmd> for SharedMessageBus {
    async fn spawn_listen(&self, routes: fn(Cmd)) -> &SharedMessageBus {
        let mut receiver = self.receiver::<Cmd>().await;

        tokio::spawn(async move {
            loop {
                if let Ok(msg) = receiver.recv().await {
                    _ = routes(msg);
                }
            }
        });
        self
    }
}

pub trait Shooter<Cmd> {
    async fn shoot(&self, cmd: Cmd) -> Result<()>;
}

impl<Cmd: Clone + Send + Sync + 'static> Shooter<Cmd> for SharedMessageBus {
    async fn shoot(&self, cmd: Cmd) -> Result<()> {
        _ = self.sender().await.send(cmd);
        Ok(())
    }
}
