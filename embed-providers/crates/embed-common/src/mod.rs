pub mod durability;
pub mod error;
pub mod provider;
pub mod logging;
pub mod rate_limit;
#[cfg(test)]
pub mod testing;
mod benchmarking;

pub use error::EmbeddingError;
pub use provider::{EmbeddingProvider, EmbeddingConfig};
pub use rate_limit::RateLimiter;