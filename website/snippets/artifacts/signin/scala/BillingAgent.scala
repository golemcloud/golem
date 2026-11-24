@agentDefinition(
  // Golem checks the user's sign-in before any code here runs:
  mount = "/billing/{org}", auth = true, //@
)
trait BillingAgent extends BaseAgent:
  class Id(val org: String)
  @endpoint("POST", "/ask")
  def ask(question: String, ctx: InvocationContext): Future[String]

@agentImplementation()
final class BillingAgentImpl(org: String) extends BillingAgent:
  def ask(question: String, ctx: InvocationContext)
      : Future[String] =
    // Golem passes in the user who signed in:
    ctx.caller.original match //@
      case user: Principal.User =>
        // the issuer can be Google, GitLab or any OIDC provider:
        val (issuer, subject) = (user.issuer, user.subject) //@
        // the model makes a page from that user's own invoices:
        invoicesOf(org, issuer, subject).flatMap: invoices =>
          pageFor(question, invoices)
      case _ => Future.failed(Exception("sign-in required"))
