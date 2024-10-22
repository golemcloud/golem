use golem_common::golem_version;

pub mod cloud;

pub const VERSION: &str = golem_version!();

#[cfg(test)]
test_r::enable!();
