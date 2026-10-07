use super::*;

mod compute;
mod configuration;
mod monitor;
mod storage;

test_r::tag_suite!(compute, group5);
test_r::tag_suite!(monitor, group5);
test_r::tag_suite!(storage, group5);
