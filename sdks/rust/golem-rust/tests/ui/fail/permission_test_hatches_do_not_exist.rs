use golem_rust::permissions::{CardId, ScopeCard};

fn reveal(id: &CardId) {
    let _ = id.__reveal_for_tests();
}

fn raw_handle(card: &ScopeCard) {
    let _ = card.__raw_handle_for_host_api_tests();
}

fn main() {
    let _ = (reveal, raw_handle);
}
