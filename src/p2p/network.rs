use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::Sender;

use crate::{ctx::CtxCommand, model::Transaction};

#[derive(Debug, Serialize, Deserialize)]
pub struct Version {
    pub id: u16,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum NetMessage {
    Version(Version),
    Verack,
    Ping,
    Pong,
    NewTransaction(Transaction),
}

impl NetMessage {
    pub fn as_binary(&self) -> Vec<u8> {
        bincode::serialize(&self).expect("Could not serialize NetMessage")
    }

    pub async fn handle(self, ctx: &Sender<CtxCommand>) {
        match self {
            NetMessage::Ping => todo!(),
            NetMessage::Pong => todo!(),
            NetMessage::NewTransaction(tx) => {
                ctx.send(CtxCommand::AddTransaction(tx)).await.unwrap();
            }
            NetMessage::Version(_) => todo!(),
            NetMessage::Verack => todo!(),
        }
    }
}
