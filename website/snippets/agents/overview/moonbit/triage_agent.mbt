#derive.golem_schema
struct Email { id : String; body : String }
#derive.golem_schema
enum Label { Urgent; Billing; Other }

#derive.agent
struct TriageAgent {
  inbox : String
}

// each inbox gets its own agent, in its own WebAssembly VM:
fn TriageAgent::new(inbox : String) -> TriageAgent {
  { inbox, }
}

pub async fn TriageAgent::triage(
  self : Self,
  emails : Array[Email], //@
) -> Map[String, Label] {
  let labels : Map[String, Label] = {}
  for email in emails {
    let label = classify(email.body)
    // it resumes here after a crash, with memory and files:
    apply_label(self.inbox, email.id, label) //@
    labels[email.id] = label
  }
  labels
}
