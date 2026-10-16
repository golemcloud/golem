use golem_rust::agent_client;

#[agent_client]
trait ReservedWithScopeCard {
    fn with_scope_card(&self);
}

#[agent_client]
trait ReservedSetDefault {
    fn set_default_scope_card(&self);
}

#[agent_client]
trait ReservedClearDefault {
    fn clear_default_scope_card(&self);
}

fn main() {}
