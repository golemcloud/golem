// asked to export results as CSV, the agent replies with a link:
// the link is /exports/<chat>/<random id>.csv, for that user only:
@httpRouter(typeName = "ExportsRouter", mount = "/exports")
trait ExportsRouter extends BaseAgent:
  @httpHandler def serve(request: HttpRequest): Future[HttpResponse]

@agentImplementation()
final class ExportsRouterImpl extends ExportsRouter:
  private val Csv = """^/exports/([\w-]+)/([\w-]{36})\.csv$""".r

  def serve(request: HttpRequest): Future[HttpResponse] =
    (request.method, request.path) match //@
      case ("GET", Csv(chat, id)) =>
        // that user's agent builds the CSV now and streams it:
        AnalystAgentClient.get(chat).exportCsv(id).map: csv => //@
          HttpResponse(UShort(200), List(
            HttpHeader.ascii("content-type", "text/csv"), //@
            // the browser saves it as a download:
            HttpHeader.ascii("content-disposition", //@
              """attachment; filename="results.csv""""), //@
          ), csv) //@
      case _ => //@
        Future.successful(HttpResponse(UShort(404), Nil, empty())) //@
