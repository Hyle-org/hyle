use std::{
    collections::HashMap,
    fmt::{self, Debug, Display},
};

use anyhow::{Error, Result};
use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::{
    p2p::network::{ReplicaRegistryNetMessage, Signed},
    utils::crypto::BlstCrypto,
};

#[derive(Serialize, Deserialize, Clone, Encode, Decode, Default)]
pub struct ReplicaPubKey(pub Vec<u8>);

impl std::fmt::Debug for ReplicaPubKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ReplicaPubKey")
            .field(&hex::encode(&self.0))
            .finish()
    }
}

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

    pub fn handle_net_message(&mut self, msg: ReplicaRegistryNetMessage) {
        match msg {
            ReplicaRegistryNetMessage::NewReplica(r) => self.add_replica(r.id.clone(), r),
        }
    }

    fn add_replica(&mut self, id: ReplicaId, replica: Replica) {
        info!("Adding replica '{}'", id);
        debug!("{:?}", replica);
        self.replicas.insert(id, replica);
    }

    pub fn check_signed<T>(&self, msg: &Signed<T>) -> Result<bool, Error>
    where
        T: Encode + Debug,
    {
        let replica = self.replicas.get(&msg.replica_id);
        match replica {
            Some(r) => BlstCrypto::verify(msg, &r.pub_key),
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
