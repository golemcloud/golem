final case class Email(id: String, body: String) derives Schema
enum Label derives Schema:
  case Urgent, Billing, Other

@agentDefinition()
trait TriageAgent extends BaseAgent:
  // each inbox gets its own agent, in its own WebAssembly VM:
  class Id(val inbox: String)
  def triage(emails: List[Email]): Future[Map[String, Label]] //@

@agentImplementation()
final class TriageAgentImpl(inbox: String) extends TriageAgent:
  def triage(emails: List[Email]): Future[Map[String, Label]] =
    emails.foldLeft(Future.successful(Map.empty[String, Label])):
      (labels, email) =>
        for
          done <- labels
          label <- classify(email.body).map(Label.valueOf)
          // it resumes here after a crash, with memory and files:
          _ <- applyLabel(inbox, email.id, label) //@
        yield done + (email.id -> label)
