@toolDefinition()
trait Summarize:
  def summarize(url: String): Future[String]

@toolImplementation()
final class SummarizeImpl extends Summarize:
  // each call gets a fresh VM that can't read the agent's memory:
  def summarize(url: String): Future[String] = //@
    // recovery reuses this completed fetch's response:
    fetch(url).toFuture.flatMap(_.text().toFuture) //@
      .flatMap(summarizeText)

@agentDefinition()
trait DigestAgent extends BaseAgent:
  class Id(val topic: String)
  def build(urls: Urls): Future[Urls]

@agentImplementation()
final class DigestAgentImpl(topic: String) extends DigestAgent:
  def build(urls: Urls): Future[Urls] =
    urls.foldLeft(Future.successful(List.empty[String])):
      (out, url) =>
        for
          done <- out
          summary <- SummarizeClient().summarize(url).map(_.orThrow)
        yield done :+ summary
