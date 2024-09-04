use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::fs;

#[derive(Serialize, Deserialize, Debug)]
pub struct Storage {
    pub interval: u64,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Config {
    peers: Vec<String>,
    pub storage: Storage,
}

impl Config {
    pub fn addr(&self, id: usize) -> Option<&str> {
        return self.peers.get(id).map(String::as_str);
    }
}

pub async fn read(path: &str) -> Result<Config> {
    let config_data = fs::read_to_string(path)
        .await
        .context("reading config file")?;

    ron::from_str(&config_data).context("deserializing config file")
}
