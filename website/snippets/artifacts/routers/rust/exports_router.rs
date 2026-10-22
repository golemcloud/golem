// this router serves exports from durable agents:
struct ExportsRouter;
#[http_router(name = "ExportsRouter", mount = "/exports")]
impl HttpRouter for ExportsRouter {
  type Config = ();
  fn new(_: Config<()>) -> Self { Self }
  async fn handle(&self, request: HttpRequest) -> HttpResponse {
    let id = request.path.strip_prefix("/exports/") //@
      .and_then(|rest| rest.strip_suffix(".csv"))
      .filter(|id| !id.is_empty() && id.chars().all(is_word));
    let Some(id) = id.filter(|_| request.method == "GET") else { //@
      let body = empty();
      return HttpResponse { status: 404, headers: vec![], body }; //@
    };
    // the durable agent behind each export streams its CSV:
    let rows = ExportAgentClient::get(id.to_string()).csv().await; //@
    HttpResponse {
      status: 200,
      headers: vec![
        header("content-type", "text/csv"),
        header("content-disposition", "attachment"), //@
      ],
      body: rows, //@
    }
  }
}
