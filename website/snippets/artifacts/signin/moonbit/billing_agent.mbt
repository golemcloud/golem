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
  self : Self, question : String,
  // Golem passes in the user who signed in:
  caller : @common.Principal, //@
) -> String {
  guard caller is Oidc(oidc) else { fail("sign-in required") }
  // the issuer can be Google, GitLab or any OIDC provider:
  let { issuer, sub, .. } = oidc //@
  // the model makes a page from that user's own invoices:
  let invoices = invoices_of(self.org, issuer, sub)
  page_for(question, invoices)
}
