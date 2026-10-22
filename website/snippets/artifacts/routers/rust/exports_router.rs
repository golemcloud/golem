struct ExportsRouter;
// asked to export results as CSV, the agent replies with a link:
// the link is /exports/<chat>/<random id>.csv, for that user only:
#[http_router(name = "ExportsRouter", mount = "/exports")]
impl HttpRouter for ExportsRouter {
  type Config = ();
  fn new(_: Config<()>) -> Self { Self }
  async fn handle(&self, request: HttpRequest) -> HttpResponse {
    export(request).await.unwrap_or_else(failed)
  }
}
async fn export(request: HttpRequest)
  -> Result<HttpResponse, AppError> {
  let path = Regex::new(r"^/exports/([\w-]+)/([\w-]{36})\.csv$")?;
  let m = path.captures(&request.path); //@
  let Some(m) = m.filter(|_| request.method == "GET") else { //@
    return Ok(HttpResponse { status: 404, headers: vec![], //@
      body: empty() });
  };
  // that user's agent builds the CSV now and streams it:
  let csv = AnalystAgentClient::get(m[1].to_string())? //@
    .export_csv(m[2].to_string()).await?;
  Ok(HttpResponse { status: 200, headers: vec![
    header("content-type", "text/csv"), //@
    // the browser saves it as a download:
    header("content-disposition", //@
      "attachment; filename=\"results.csv\""), //@
  ], body: csv }) //@
}
