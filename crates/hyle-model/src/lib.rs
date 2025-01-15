#[cfg(feature = "full")]
mod block;
#[cfg(feature = "full")]
mod node;
#[cfg(feature = "full")]
mod transaction;
#[cfg(feature = "full")]
pub mod utils;

#[cfg(feature = "full")]
pub use block::*;
#[cfg(feature = "full")]
pub use node::*;
#[cfg(feature = "full")]
pub use transaction::*;

mod contract;
mod staking;
pub use contract::*;
pub use staking::*;

pub const HASH_DISPLAY_SIZE: usize = 3;
