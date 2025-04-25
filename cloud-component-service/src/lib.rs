use golem_common::golem_version;

pub mod api;
pub mod config;
pub mod grpcapi;
pub mod metrics;
pub mod model;
pub mod service;

#[cfg(test)]
test_r::enable!();

pub const VERSION: &str = golem_version!();
