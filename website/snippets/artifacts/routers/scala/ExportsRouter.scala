// this router serves exports from durable agents:
@httpRouter("ExportsRouter", "/exports")
trait ExportsRouter extends BaseAgent:
  @httpHandler def serve(request: HttpRequest): Future[HttpResponse]

@agentImplementation()
final class ExportsRouterImpl() extends ExportsRouter:
  private val Csv = """/exports/([\w-]+)\.csv""".r //@

  def serve(request: HttpRequest): Future[HttpResponse] =
    (request.method, request.path) match
      case ("GET", Csv(id)) =>
        // the durable agent behind each export streams its CSV:
        ExportAgentClient.get(id).csv().map: rows => //@
          HttpResponse(UShort(200), List(
            HttpHeader.ascii("content-type", "text/csv"),
            HttpHeader.ascii("content-disposition", "attachment"), //@
          ), rows) //@
      case _ => //@
        Future.successful(HttpResponse(UShort(404), Nil, empty())) //@
