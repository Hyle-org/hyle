use std::{path::PathBuf, sync::Arc};

use anyhow::Result;
use config::{Config, ConfigError, Environment, File};
use serde::{Deserialize, Serialize};

use crate::validator_registry::ValidatorId;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Storage {
    pub interval: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Consensus {
    pub slot_duration: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct P2pConf {
    pub ping_interval: u64,
}
pub type SharedConf = Arc<Conf>;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Conf {
    port: u16,
    host: String,
    pub id: ValidatorId,
    pub peers: Vec<String>,
    pub storage: Storage,
    pub consensus: Consensus,
    rest: String,
    pub p2p: P2pConf,
    pub data_directory: PathBuf,
}

impl Conf {
    pub fn addr(&self) -> (&str, u16) {
        (&self.host, self.port)
    }

    pub fn rest_addr(&self) -> &str {
        return self.rest.as_str();
    }

    pub fn history_db_path(&self) -> PathBuf {
        self.data_directory.join("history.db")
    }

    pub fn new(config_file: String, data_directory: Option<String>) -> Result<Self, ConfigError> {
        let s = Config::builder()
            // Priority order: config file, then environment variables, then CLI
            .add_source(File::with_name(config_file.as_str()))
            .add_source(Environment::with_prefix("hyle"))
            .set_override_option("data_directory", data_directory)?
            .build()?;

        // You can deserialize (and thus freeze) the entire configuration as
        s.try_deserialize()
    }

    pub fn new_shared(
        config_file: String,
        data_directory: Option<String>,
    ) -> Result<SharedConf, ConfigError> {
        Self::new(config_file, data_directory).map(Arc::new)
    }
}
