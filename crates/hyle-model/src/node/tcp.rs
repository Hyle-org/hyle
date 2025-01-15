use anyhow::{Context, Result};
use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};
use std::fmt::Display;
use strum_macros::IntoStaticStr;

use crate::*;

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, Eq, PartialEq, IntoStaticStr)]
pub enum TcpServerNetMessage {
    Ping,
    NewTx(Transaction),
}

impl Display for TcpServerNetMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let enum_variant: &'static str = self.into();
        match self {
            TcpServerNetMessage::NewTx(msg) => {
                _ = write!(f, "TcpServerMessage::{} ", enum_variant);
                write!(f, "{:?}", msg)
            }
            TcpServerNetMessage::Ping => {
                _ = write!(f, "TcpServerMessage::{} ", enum_variant);
                write!(f, "{}", enum_variant)
            }
        }
    }
}

impl From<Transaction> for TcpServerNetMessage {
    fn from(msg: Transaction) -> Self {
        TcpServerNetMessage::NewTx(msg)
    }
}

impl From<BlobTransaction> for TcpServerNetMessage {
    fn from(msg: BlobTransaction) -> Self {
        let tx: Transaction = msg.into();
        tx.into()
    }
}

impl From<ProofTransaction> for TcpServerNetMessage {
    fn from(msg: ProofTransaction) -> Self {
        let tx: Transaction = msg.into();
        tx.into()
    }
}

impl From<RegisterContractTransaction> for TcpServerNetMessage {
    fn from(msg: RegisterContractTransaction) -> Self {
        let tx: Transaction = msg.into();
        tx.into()
    }
}

impl TcpServerNetMessage {
    pub fn to_binary(&self) -> Result<Vec<u8>> {
        bincode::encode_to_vec(self, bincode::config::standard())
            .context("Could not serialize NetMessage")
    }
}
