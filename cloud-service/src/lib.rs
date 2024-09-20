use golem_common::golem_version;

pub mod api;
pub mod auth;
pub mod config;
pub mod grpcapi;
pub mod metrics;
pub mod model;
pub mod repo;
pub mod service;

const VERSION: &str = golem_version!();
