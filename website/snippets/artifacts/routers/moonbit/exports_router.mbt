// asked to export results as CSV, the agent replies with a link:
// the router sets the status, content type and download headers:
#derive.http_router
#derive.mount("/exports")
struct ExportsRouter {}
fn ExportsRouter::new() -> ExportsRouter { ExportsRouter::{ } }

#derive.http_handler
pub async fn ExportsRouter::serve(
  _self : Self, request : @http.HttpRequest,
) -> @http.HttpResponse {
  guard request.method == "GET" && //@
    request.path is [.. "/exports/", .. rest, .. ".csv"] && //@
    rest.split("/").collect() is [chat, id] &&
    is_word(chat) && is_word(id) && id.length() == 36 else {
    return { status: 404, headers: [], body: empty() } //@
  }
  // that user's agent builds the CSV now and streams it:
  let csv = AnalystAgentClient::get(chat.to_string()) //@
    .export_csv(id.to_string())
  {
    status: 200,
    headers: [
      header("content-type", "text/csv"), //@
      // the browser saves it as a download:
      header("content-disposition", //@
        "attachment; filename=\"results.csv\""), //@
    ],
    body: csv, //@
  }
}
