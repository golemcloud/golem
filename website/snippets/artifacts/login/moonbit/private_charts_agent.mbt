struct Png {}
impl @schema.AllowedMimeTypes for Png with fn mime_types() {
  ["image/png"]
}

#derive.agent
#derive.mount("/private/{team}")
#derive.mount_auth(true)
struct PrivateChartsAgent { team : String; charts : Array[Chart] }

fn PrivateChartsAgent::new(team : String) -> PrivateChartsAgent {
  { team, charts: [] }
}

#derive.endpoint(post="/ask")
pub async fn PrivateChartsAgent::ask(self : Self, question : String,
  caller : @common.Principal) -> Int {
  // the agent charts a signed-in user's question for that user:
  let png = answer_with_chart(question)
  self.charts.push({ asked_by: user_key(caller), png }) //@
  self.charts.length()
}

#derive.endpoint(get="/charts/{n}")
pub async fn PrivateChartsAgent::open(self : Self, n : Int,
  caller : @common.Principal) -> @schema.UnstructuredBinary[Png]? {
  guard self.charts.get(n - 1) is Some(made) else { return None }
  // anyone else, even signed in, gets a 404:
  guard made.asked_by == user_key(caller) else { return None } //@
  Some(Inline(bytes=made.png, mime_type=Some("image/png")))
}
