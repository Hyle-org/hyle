use anyhow::Result;
use config::{Config, ConfigError, Environment, File};
use serde::{Deserialize, Serialize};
use std::{fmt::Debug, path::PathBuf, sync::Arc};

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
    pub id: String,
    pub host: String,
    pub peers: Vec<String>,
    pub storage: Storage,
    pub consensus: Consensus,
    pub rest: String,
    pub database_url: String,
    pub p2p: P2pConf,
    pub data_directory: PathBuf,
    pub run_indexer: bool,
    pub da_address: String,
    pub log_format: String,
}

impl Conf {
    pub fn new(
        config_file: Option<String>,
        data_directory: Option<String>,
        run_indexer: Option<bool>,
    ) -> Result<Self, ConfigError> {
        let mut s = Config::builder().add_source(File::from_str(
            include_str!("conf_defaults.ron"),
            config::FileFormat::Ron,
        ));
        // Priority order: config file, then environment variables, then CLI
        if let Some(config_file) = config_file {
            s = s.add_source(File::with_name(&config_file).required(false));
        }
        s.add_source(
            Environment::with_prefix("hyle")
                .separator("__")
                .prefix_separator("_")
                .list_separator(",")
                .with_list_parse_key("peers") // Parse this key into Vec<String>
                .try_parsing(true),
        )
        .set_override_option("data_directory", data_directory)?
        .set_override_option("run_indexer", run_indexer)?
        .build()?
        .try_deserialize()
    }
}
