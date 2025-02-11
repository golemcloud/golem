use crate::Tracing;
use golem_test_framework::config::EnvBasedTestDependencies;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
async fn launch_new_worker(deps: &EnvBasedTestDependencies) {}
