// Golem checks the user's sign-in before any code here runs:
#[agent_definition(mount = "/billing/{org}", auth = true)] //@
pub trait BillingAgent {
  fn new(org: String) -> Self;
  #[endpoint(post = "/ask")]
  // Golem passes in the user who signed in:
  async fn ask(&self, question: String, caller: Principal) //@
    -> Result<String, AppError>;
}
struct BillingAgentImpl { org: String }
#[agent_implementation]
impl BillingAgent for BillingAgentImpl {
  fn new(org: String) -> Self { Self { org } }
  async fn ask(&self, question: String, caller: Principal)
    -> Result<String, AppError> {
    let Principal::Oidc(user) = caller else {
      return Err("sign-in required".into());
    };
    // the issuer can be Google, GitLab or any OIDC provider:
    let (issuer, sub) = (user.issuer, user.sub); //@
    // the model makes a page from that user's own invoices:
    let invoices = invoices_of(&self.org, &issuer, &sub).await?;
    page_for(&question, &invoices).await
  }
}
