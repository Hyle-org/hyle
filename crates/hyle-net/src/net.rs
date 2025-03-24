#[cfg(not(feature = "turmoil"))]
pub use tokio::net::*;

#[cfg(feature = "turmoil")]
pub use turmoil::net::*;
#[cfg(feature = "turmoil")]
pub use turmoil::*;
