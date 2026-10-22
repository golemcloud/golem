def bytes(text: String): Array[Byte] = text.getBytes("UTF-8")

@httpRouter("ExportsRouter", "/exports")
trait ExportsRouter extends BaseAgent:
  @httpHandler def serve(request: HttpRequest): Future[HttpResponse]

@agentImplementation()
final class ExportsRouterImpl() extends ExportsRouter:
  private val Csv = """/exports/([\w-]+)\.csv""".r

  def serve(request: HttpRequest): Future[HttpResponse] =
    (request.method, request.path) match
      case ("GET", Csv(id)) =>
        // stream the CSV from the agent that builds this export:
        ExportAgentClient.get(id).csv().map: rows => //@
          HttpResponse(UShort(200), List(
            HttpHeader("content-type", bytes("text/csv")), //@
            // browsers save it as a file instead of opening it:
            HttpHeader("content-disposition", bytes( //@
              s"""attachment; filename="$id.csv"""")), //@
          ), rows)
      case _ =>
        val body = AgentStream.fromStream(Stream.empty)
        Future.successful(HttpResponse(UShort(404), Nil, body))
