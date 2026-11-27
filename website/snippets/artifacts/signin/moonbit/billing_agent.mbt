#derive.agent
#derive.mount("/billing/{org}")
// Golem checks the user's sign-in before any code here runs:
#derive.mount_auth(true) //@
struct BillingAgent { org : String }

fn BillingAgent::new(org : String) -> BillingAgent {
  { org, }
}

#derive.endpoint(post="/ask")
pub async fn BillingAgent::ask(
  self : Self, question : String, ctx : @agents.InvocationContext,
) -> String {
  // Golem passes in the user who signed in:
  guard ctx.caller().original is User(user) else { //@
    fail("sign-in required")
  }
  // the issuer can be Google, GitLab or any OIDC provider:
  let { issuer, subject } = user.principal_ref //@
  // the model makes a page from that user's own invoices:
  let invoices = invoices_of(self.org, issuer, subject)
  page_for(question, invoices)
}
