use super::*;

mod admission;
mod health;
mod reconstruction;
mod retirement;
mod windows;

test_r::tag_suite!(admission, group5);
test_r::tag_suite!(health, group5);
test_r::tag_suite!(reconstruction, group5);
test_r::tag_suite!(retirement, group5);
test_r::tag_suite!(windows, group5);
