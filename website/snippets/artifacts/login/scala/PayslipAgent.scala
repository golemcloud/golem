type Pdf = UnstructuredBinary["application/pdf"]

@agentDefinition(
  mount = "/payslips/{employee}",
  // Golem checks the sign-in token before any agent code runs:
  auth = true, //@
)
trait PayslipAgent extends BaseAgent:
  class Id(val employee: String)
  @endpoint("GET", "/latest.pdf")
  def latest(caller: Principal): Option[Pdf]

@agentImplementation()
final class PayslipAgentImpl(employee: String) extends PayslipAgent:
  def latest(caller: Principal): Option[Pdf] =
    // the agent decides who may open the payslip:
    caller match
      case login: Principal.Oidc if login.sub == employee => //@
        val pdf = fs.readFileSync("/payslips/latest.pdf")
        val data = pdf.toArray.map(_.toByte)
        Some(UnstructuredBinary.fromInline(data, "application/pdf"))
      case _ => None
