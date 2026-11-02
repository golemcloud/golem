use super::*;

mod http;
mod promise;
mod tcp;
mod udp;

test_r::tag_suite!(http, group5);
test_r::tag_suite!(promise, group5);
test_r::tag_suite!(udp, group5);
