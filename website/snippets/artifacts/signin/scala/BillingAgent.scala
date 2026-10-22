@agentDefinition(
  // Golem checks the user's sign-in before any code here runs:
  mount = "/billing/{org}", auth = true, //@
)
trait BillingAgent extends BaseAgent:
  class Id(val org: String)
  @endpoint("POST", "/ask")
  // Golem passes in the user who signed in:
  def ask(question: String, caller: Principal): Future[String] //@

@agentImplementation()
final class BillingAgentImpl(org: String) extends BillingAgent:
  def ask(question: String, caller: Principal): Future[String] =
    caller match
      case user: Principal.Oidc =>
        // the issuer can be Google, GitLab or any OIDC provider:
        val (issuer, sub) = (user.issuer, user.sub) //@
        // the model makes a page from that user's own invoices:
        invoicesOf(org, issuer, sub).flatMap: invoices =>
          pageFor(question, invoices)
      case _ => Future.failed(Exception("sign-in required"))
