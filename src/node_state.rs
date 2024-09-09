#![allow(dead_code, unused_variables)]
use anyhow::{Error, Result};

use model::NodeState;
mod model;

impl NodeState {
    pub fn publish_payloads(&self) -> Result<(), Error> {
        Ok(())
    }
}
