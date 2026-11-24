#derive.agent
#derive.mount("/private/{team}")
#derive.mount_auth(true)
struct PrivateChartsAgent { team : String; charts : Array[Chart] }

fn PrivateChartsAgent::new(team : String) -> PrivateChartsAgent {
  { team, charts: [] }
}

#derive.endpoint(post="/ask")
pub async fn PrivateChartsAgent::ask(self : Self, question : String,
  ctx : @agents.InvocationContext) -> Int {
  // the agent charts a signed-in user's question for that user:
  let png = answer_with_chart(question)
  let asked_by = ctx.caller().original.principal_ref()
  self.charts.push({ asked_by, png }) //@
  self.charts.length()
}

#derive.endpoint(get="/charts/{n}")
pub async fn PrivateChartsAgent::open(
  self : Self, n : Int, ctx : @agents.InvocationContext,
) -> @schema.UnstructuredBinary[Png]? {
  guard self.charts.get(n - 1) is Some(made) else { return None }
  let me = ctx.caller().original.principal_ref()
  // anyone else, even signed in, gets a 404:
  guard me is Some(_) && made.asked_by == me else { return None } //@
  Some(Inline(bytes=made.png, mime_type=Some("image/png")))
}
