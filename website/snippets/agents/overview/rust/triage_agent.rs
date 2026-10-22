#[derive(Schema)] pub struct Email { id: String, body: String }
#[derive(Schema, Clone, Copy)]
pub enum Label { Urgent, Billing, Other }

#[agent_definition]
pub trait TriageAgent {
  // each inbox gets its own agent, in its own WebAssembly VM:
  fn new(inbox: String) -> Self;
  async fn triage(&self, emails: Vec<Email>) //@
    -> Result<HashMap<String, Label>, AppError>;
}
struct TriageAgentImpl { inbox: String }
#[agent_implementation]
impl TriageAgent for TriageAgentImpl {
  fn new(inbox: String) -> Self { Self { inbox } }
  async fn triage(&self, emails: Vec<Email>)
    -> Result<HashMap<String, Label>, AppError> {
    let mut labels = HashMap::new();
    for email in emails {
      let label: Label = classify(&email.body).await?;
      // it resumes here after a crash, with memory and files:
      apply_label(&self.inbox, &email.id, label).await?; //@
      labels.insert(email.id, label);
    }
    Ok(labels)
  }
}
