#[cfg(feature = "risc0")]
pub use risc0_sha2::*;
#[cfg(not(any(feature = "risc0", feature = "sp1")))]
pub use sha2::{Digest, Sha256};
#[cfg(feature = "sp1")]
pub use sp1_sha2::*;
