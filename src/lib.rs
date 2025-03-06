//! # Hylé
//!
//! *A sequencing and settlement layer to help you build provable apps that are minimally, yet sufficiently, onchain.*
//!
//! Repository for the [Hylé](https://hyle.eu) chain. This repository is for the work-in-progress rust client.
//! The older (but still maintained) Cosmos SDK based client can be found at [hyle-cosmos](https://github.com/Hyle-org/hyle-cosmos).
//!
//! **Current status**: Work in progress
//!
//! ## Shortcuts to important pages
//!
//! * [Hyle contract sdk](../hyle_contract_sdk)
//!
//! ## Useful links
//!
//! * [Hylé website](https://www.hyle.eu/)
//! * [Hylé documentation](https://docs.hyle.eu)
//!
//!

pub mod bus;
pub mod consensus;
pub mod data_availability;
pub mod genesis;
pub mod indexer;
pub mod mempool;
pub mod node_state;
pub mod p2p;
pub mod rest;
pub mod single_node_consensus;
pub mod tcp_server;
pub mod utils;

#[cfg(test)]
pub mod tests;

pub mod model;
pub mod tools;
