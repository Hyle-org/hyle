use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::fs;

#[derive(Serialize, Deserialize, Debug)]
pub struct Config {
    rpc_addr: String,
    rest_addr: String,
}

impl Config {
    pub fn rpc_addr(&self) -> &str {
        return &self.rpc_addr;
    }

    pub fn rest_addr(&self) -> &str {
        return &self.rest_addr;
    }
}

pub async fn read(path: &str) -> Result<Config> {
    let config_data = fs::read_to_string(path)
        .await
        .context("reading config file")?;

    ron::from_str(&config_data).context("deserializing config file")
}
