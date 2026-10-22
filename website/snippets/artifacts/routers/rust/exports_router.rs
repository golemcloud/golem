struct ExportsRouter;
#[http_router(name = "ExportsRouter", mount = "/exports")]
impl HttpRouter for ExportsRouter {
  type Config = ();
  fn new(_: Config<()>) -> Self { Self }
  async fn handle(&self, request: HttpRequest) -> HttpResponse {
    let id = request.path.strip_prefix("/exports/")
      .and_then(|rest| rest.strip_suffix(".csv"))
      .filter(|id| !id.is_empty() && id.chars().all(is_word));
    let Some(id) = id.filter(|_| request.method == "GET") else {
      let body = AgentStream::new().1;
      return HttpResponse { status: 404, headers: vec![], body };
    };
    // stream the CSV from the agent that builds this export:
    let rows = ExportAgentClient::get(id.to_string()).csv().await; //@
    HttpResponse {
      status: 200,
      headers: vec![
        Header { name: "content-type".into(), //@
          value: b"text/csv".to_vec() }, //@
        // browsers save it as a file instead of opening it:
        Header { name: "content-disposition".into(), value: //@
          format!("attachment; filename=\"{id}.csv\"") //@
            .into_bytes() },
      ],
      body: rows,
    }
  }
}
