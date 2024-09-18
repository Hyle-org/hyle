use std::{collections::HashMap, fmt::Display};

use anyhow::{Error, Result};
use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::p2p::network::{ReplicaRegistryNetMessage, Signed};

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, Default)]
pub struct ReplicaPubKey(pub Vec<u8>);

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, Default, Hash, Eq, PartialEq)]
pub struct ReplicaId(pub String);

impl Display for ReplicaId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode)]
pub struct Replica {
    pub id: ReplicaId,
    pub pub_key: ReplicaPubKey,
}

#[derive(Serialize, Deserialize, Encode, Decode)]
pub struct ReplicaRegistry {
    pub replicas: HashMap<ReplicaId, Replica>,
}

impl ReplicaRegistry {
    pub fn new() -> ReplicaRegistry {
        ReplicaRegistry {
            replicas: Default::default(),
        }
    }

    pub async fn handle_net_message(&mut self, msg: ReplicaRegistryNetMessage) {
        match msg {
            ReplicaRegistryNetMessage::NewReplica(r) => self.add_replica(r.id.clone(), r),
        }
    }

    fn add_replica(&mut self, id: ReplicaId, replica: Replica) {
        self.replicas.insert(id, replica);
    }

    pub fn check_signed<T: Encode>(&self, msg: &Signed<T>) -> Result<bool, Error> {
        let replica = self.replicas.get(&msg.replica_id);
        match replica {
            Some(r) => {
                let _encoded = bincode::encode_to_vec(&msg.msg, bincode::config::standard())?;
                let _signature = &msg.signature;
                let _pub_key = &r.pub_key;

                // TODO: check if signature is valid for pub_key

                Ok(true)
            }
            None => {
                warn!("Replica '{}' not found", msg.replica_id);
                Ok(false)
            }
        }
    }
}

impl Default for ReplicaRegistry {
    fn default() -> Self {
        Self::new()
    }
}
