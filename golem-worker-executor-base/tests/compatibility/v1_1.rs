use goldenfile::Mint;
use golem_worker_executor_base::error::GolemError;

#[test]
pub fn golem_error() {
    let g1 = GolemError::ShardingNotReady;

    let mut mint = Mint::new("tests/goldenfiles");
    crate::compatibility::v1::backward_compatible("golem_error_sharding_not_ready", &mut mint, g1);
}
