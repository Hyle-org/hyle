use anyhow::{Context, Result};
use config::{Config, Environment, File};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use serde_with::DurationMilliSeconds;
use std::{collections::HashMap, fmt::Debug, path::PathBuf, sync::Arc, time::Duration};

#[serde_as]
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Consensus {
    #[serde_as(as = "DurationMilliSeconds")]
    pub slot_duration: Duration,
    /// Whether the network runs as a single node or with a multi-node consensus.
    pub solo: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct GenesisConf {
    /// Initial bonded stakers and their stakes
    pub stakers: HashMap<String, u64>,
    /// Faucer configuration
    pub faucet_password: String,
}

/// Configuration for the P2P layer
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct P2pConf {
    pub mode: P2pMode,
    /// Server address for the P2P layer
    pub server_port: u16,
    /// IPs of peers to connect to
    pub peers: Vec<String>,
    /// Time in milliseconds between pings to peers
    pub ping_interval: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub enum P2pMode {
    /// Run a full node with a validator participating in consensus
    FullValidator,
    /// Run a full node without consensus (assumes you have your own lane)
    LaneManager,
    /// Run a limited node that subscribes to another one for DA
    #[default]
    None,
}

pub type SharedConf = Arc<Conf>;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Conf {
    /// Human-readable identifier for this node.
    pub id: String,

    // Network host name
    pub hostname: String,
    /// The log format to use - "json", "node" or "full" (default)
    pub log_format: String,
    /// Directory name to store node state.
    pub data_directory: PathBuf,

    /// Peer-to-peer layer configuration
    pub p2p: P2pConf,

    // Validator options
    /// Consensus configuration
    pub consensus: Consensus,
    /// Genesis block configuration
    pub genesis: GenesisConf,

    // Module options below
    /// If full node: server address for the DA layer, which streams historical & new blocks. It might be used by indexers.
    /// If "None", this is instead the address to connect to.
    pub da_server_port: u16,

    /// For a Da client
    pub da_address: String,

    pub run_rest_server: bool,
    /// Server port for the REST API
    pub rest_server_port: u16,
    /// Maximum body size for REST requests
    pub rest_server_max_body_size: usize,

    pub run_tcp_server: bool,
    /// Server port for the TCP API
    pub tcp_server_port: u16,

    /// Whether to run the indexer
    pub run_indexer: bool,
    /// If running the indexer, the postgres address to connect to
    pub database_url: String,
}

impl Conf {
    pub fn new(
        config_file: Option<String>,
        data_directory: Option<String>,
        run_indexer: Option<bool>,
    ) -> Result<Self, anyhow::Error> {
        let mut s = Config::builder().add_source(File::from_str(
            include_str!("conf_defaults.toml"),
            config::FileFormat::Toml,
        ));
        // Priority order: config file, then environment variables, then CLI
        if let Some(config_file) = config_file {
            s = s.add_source(File::with_name(&config_file).required(false));
        }
        let mut conf: Self = s
            .add_source(
                Environment::with_prefix("hyle")
                    .separator("__")
                    .prefix_separator("_")
                    .list_separator(",")
                    .with_list_parse_key("p2p.peers") // Parse this key into Vec<String>
                    .try_parsing(true),
            )
            .set_override_option("data_directory", data_directory)?
            .set_override_option(
                "tcp_server_port",
                std::env::var("HYLE_TCP__SERVER__PORT")
                    .ok()
                    .and_then(|port| port.parse::<u16>().ok()), // Convertir en u16 si possible
            )?
            .set_override_option(
                "rest_server_port",
                std::env::var("HYLE_REST__SERVER__PORT")
                    .ok()
                    .and_then(|port| port.parse::<u16>().ok()), // Convertir en u16 si possible
            )?
            .set_override_option(
                "da_server_port",
                std::env::var("HYLE_DA__SERVER__PORT")
                    .ok()
                    .and_then(|port| port.parse::<u16>().ok()), // Convertir en u16 si possible
            )?
            .set_override_option(
                "da_server_port",
                std::env::var("HYLE_DA__SERVER__PORT")
                    .ok()
                    .and_then(|port| port.parse::<u16>().ok()), // Convertir en u16 si possible
            )?
            .set_override(
                "hostname",
                std::env::var("HOSTNAME").unwrap_or("localhost".to_string()),
            )?
            .set_override_option("run_indexer", run_indexer)?
            .build()?
            .try_deserialize()?;
        if conf.consensus.solo {
            conf.genesis.stakers.insert(
                conf.id.clone(),
                match std::env::var("HYLE_SINGLE_NODE_STAKE") {
                    Ok(stake) => stake.parse::<u64>().context("Failed to parse stake"),
                    Err(e) => Err(Into::into(e)),
                }
                .unwrap_or(1000),
            );
        }
        Ok(conf)
    }
}

#[cfg(test)]
mod tests {
    use assertables::assert_ok;

    use super::*;

    #[test]
    fn test_load_default_conf() {
        assert_ok!(Conf::new(None, None, None));
    }

    #[test]
    fn test_override_tcp_server_port() {
        let conf = Conf::new(None, None, None).unwrap();
        assert_eq!(conf.tcp_server_port, 1414);

        std::env::set_var("HYLE_TCP__SERVER__PORT", "9090");
        let conf = Conf::new(None, None, None).unwrap();
        assert_eq!(conf.tcp_server_port, 9090);
    }
    #[test]
    fn test_override_rest_server_port() {
        let conf = Conf::new(None, None, None).unwrap();
        assert_eq!(conf.rest_server_port, 4321);
        std::env::set_var("HYLE_REST__SERVER__PORT", "9090");
        let conf = Conf::new(None, None, None).unwrap();
        assert_eq!(conf.rest_server_port, 9090);
    }
    #[test]
    fn test_override_da_server_port() {
        let conf = Conf::new(None, None, None).unwrap();
        assert_eq!(conf.da_server_port, 4141);
        std::env::set_var("HYLE_DA__SERVER__PORT", "9090");
        let conf = Conf::new(None, None, None).unwrap();
        assert_eq!(conf.da_server_port, 9090);
    }
    #[test]
    fn test_override_hostname() {
        let conf = Conf::new(None, None, None).unwrap();
        assert_eq!(conf.hostname, "localhost");
        std::env::set_var("HOSTNAME", "hyli-node");
        let conf = Conf::new(None, None, None).unwrap();
        assert_eq!(conf.hostname, "hyli-node");
    }
}
