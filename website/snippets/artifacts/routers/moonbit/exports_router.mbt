fn bytes(s : String) -> Array[Byte] { @utf8.encode(s).to_array() }

#derive.http_router
#derive.mount("/exports")
struct ExportsRouter {}

fn ExportsRouter::new() -> ExportsRouter { ExportsRouter::{ } }

#derive.http_handler
pub async fn ExportsRouter::serve(
  _self : Self, request : @http.HttpRequest,
) -> @http.HttpResponse {
  guard request.method == "GET" &&
    request.path is [.. "/exports/", .. id, .. ".csv"] &&
    is_word(id) else {
    let body = @schema.AgentStream::produce(async fn(_) {  })
    return { status: 404, headers: [], body }
  }
  // stream the CSV from the agent that builds this export:
  let rows = ExportAgentClient::get(id.to_owned()).csv() //@
  {
    status: 200,
    headers: [
      { name: "content-type", value: bytes("text/csv") }, //@
      // browsers save it as a file instead of opening it:
      { name: "content-disposition", value: bytes( //@
        "attachment; filename=\"\{id}.csv\"") }, //@
    ],
    body: rows,
  }
}
