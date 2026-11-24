// Golem checks the user's sign-in before any code here runs:
#[agent_definition(mount = "/billing/{org}", auth = true)] //@
pub trait BillingAgent {
  fn new(org: String) -> Self;
  #[endpoint(post = "/ask")]
  async fn ask(&self, question: String, ctx: InvocationContext)
    -> Result<String, AppError>;
}
struct BillingAgentImpl { org: String }
#[agent_implementation]
impl BillingAgent for BillingAgentImpl {
  fn new(org: String) -> Self { Self { org } }
  async fn ask(&self, question: String, ctx: InvocationContext)
    -> Result<String, AppError> {
    // Golem passes in the user who signed in:
    let Principal::User(user) = ctx.caller().original() else { //@
      return Err("sign-in required".into());
    };
    // the issuer can be Google, GitLab or any OIDC provider:
    let (issuer, subject) = (user.issuer(), user.subject()); //@
    // the model makes a page from that user's own invoices:
    let invoices = invoices_of(&self.org, issuer, subject).await?;
    page_for(&question, &invoices).await
  }
}
