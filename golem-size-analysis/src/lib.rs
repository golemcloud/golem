//! Component size measurement and the Rule 2 size breakdown.
//!
//! Without the `analysis` feature the crate holds only what every size consumer shares: the gzip
//! measurement, the fixture list and the fixture descriptor types.

#[cfg(test)]
test_r::enable!();

pub mod fixtures;
pub mod gzip;

#[cfg(feature = "analysis")]
pub mod attribution;
#[cfg(feature = "analysis")]
pub mod breakdown;
#[cfg(feature = "analysis")]
pub mod collect;
#[cfg(feature = "analysis")]
pub mod contract;
#[cfg(feature = "analysis")]
pub mod cores;
#[cfg(feature = "analysis")]
pub mod optimize;
#[cfg(feature = "analysis")]
pub mod report;
#[cfg(feature = "analysis")]
pub mod retention;
#[cfg(feature = "analysis")]
pub mod sections;
