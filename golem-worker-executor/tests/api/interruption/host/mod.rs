use super::*;

mod blob_filesystem;
mod db_kv;
mod entity;
mod frontend_streams;
mod http_peer;
mod p2;
mod p3;
mod rpc;
mod sleep;
mod websocket;

test_r::tag_suite!(blob_filesystem, group5);
test_r::tag_suite!(db_kv, group5);
test_r::tag_suite!(entity, group5);
test_r::tag_suite!(frontend_streams, group5);
test_r::tag_suite!(rpc, group5);
test_r::tag_suite!(sleep, group5);
test_r::tag_suite!(websocket, group5);
