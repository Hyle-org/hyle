//! # Hylé
//!
//! *A sequencing and settlement layer to help you build provable apps that are minimally, yet sufficiently, onchain.*
//!
//! Repository for the [Hylé](https://hyle.eu) chain. This repository is for the work-in-progress rust client.
//! The older (but still maintained) Cosmos SDK based client can be found at [hyle-cosmos](https://github.com/Hyle-org/hyle-cosmos).
//!
//! **Current status**: Initial commit.
//!
//! ## Useful links
//!
//! * [Hylé website](https://www.hyle.eu/)
//! * [Hylé documentation](https://docs.hyle.eu)

mod autobahn_testing;
pub mod bus;
pub mod consensus;
pub mod data_availability;
pub mod genesis;
pub mod indexer;
pub mod mempool;
pub mod model;
pub mod node_state;
pub mod p2p;
pub mod prover;
pub mod rest;
pub mod single_node_consensus;
pub mod tools;
pub mod utils;
