use std::sync::Arc;

use anyhow::Result;
use config::{Config, ConfigError, Environment, File};
use serde::{Deserialize, Serialize};

use crate::replica_registry::ReplicaId;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Storage {
    pub interval: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct P2pConf {
    pub ping_interval: u64,
}
pub type SharedConf = Arc<Conf>;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Conf {
    port: u16,
    host: String,
    pub id: ReplicaId,
    pub peers: Vec<String>,
    pub storage: Storage,
    rest: String,
    pub p2p: P2pConf,
}

impl Conf {
    pub fn addr(&self) -> (&str, u16) {
        (&self.host, self.port)
    }

    pub fn rest_addr(&self) -> &str {
        return self.rest.as_str();
    }

    pub fn new(config_file: String) -> Result<Self, ConfigError> {
        let s = Config::builder()
            // Start off by merging in the "default" configuration file
            .add_source(File::with_name(config_file.as_str()))
            .add_source(Environment::with_prefix("hyle"))
            // You may also programmatically change settings
            // .set_override("database.url", "postgres://")?
            .build()?;

        // You can deserialize (and thus freeze) the entire configuration as
        s.try_deserialize()
    }

    pub fn new_shared(config_file: String) -> Result<SharedConf, ConfigError> {
        Self::new(config_file).map(Arc::new)
    }
}
