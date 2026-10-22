// this router serves exports from durable agents:
#derive.http_router
#derive.mount("/exports")
struct ExportsRouter {}

fn ExportsRouter::new() -> ExportsRouter { ExportsRouter::{ } }

#derive.http_handler
pub async fn ExportsRouter::serve(
  _self : Self, request : @http.HttpRequest,
) -> @http.HttpResponse {
  let id = match request.path { //@
    [.. "/exports/", .. id, .. ".csv"] if is_word(id) => Some(id)
    _ => None
  }
  guard request.method == "GET" && id is Some(id) else { //@
    return { status: 404, headers: [], body: empty() } //@
  }
  // the durable agent behind each export streams its CSV:
  let rows = ExportAgentClient::get(id.to_owned()).csv() //@
  {
    status: 200,
    headers: [
      header("content-type", "text/csv"),
      header("content-disposition", "attachment"), //@
    ],
    body: rows, //@
  }
}
