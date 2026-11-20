use golem_rust::agentic::{InvocationContext, PrincipalRef};
use golem_rust::{agent_definition, agent_implementation};

#[agent_definition]
trait Audited {
    fn new(id: String, owner: PrincipalRef) -> Self;
    fn record(&mut self, ctx: InvocationContext, note: String) -> PrincipalRef;
    fn allowed(&self, who: Vec<PrincipalRef>) -> bool;
}

struct AuditedImpl {
    owner: PrincipalRef,
}

#[agent_implementation]
impl Audited for AuditedImpl {
    fn new(_id: String, owner: PrincipalRef) -> Self {
        Self { owner }
    }

    fn record(&mut self, ctx: InvocationContext, _note: String) -> PrincipalRef {
        match ctx.caller().original().principal_ref() {
            Some(principal_ref) => principal_ref.clone(),
            None => self.owner.clone(),
        }
    }

    fn allowed(&self, who: Vec<PrincipalRef>) -> bool {
        who.contains(&self.owner)
    }
}

fn main() {}
