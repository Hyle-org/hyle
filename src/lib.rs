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

#[cfg(feature = "node")]
//mod autobahn_testing;
#[cfg(feature = "node")]
pub mod bus;
#[cfg(feature = "node")]
pub mod consensus;
#[cfg(feature = "node")]
pub mod data_availability;
#[cfg(feature = "node")]
pub mod genesis;
#[cfg(feature = "node")]
pub mod indexer;
#[cfg(feature = "node")]
pub mod mempool;
#[cfg(feature = "node")]
pub mod p2p;
#[cfg(feature = "node")]
pub mod rest;
#[cfg(feature = "node")]
pub mod single_node_consensus;
#[cfg(feature = "node")]
pub mod utils;

pub mod model;
pub mod tools;
