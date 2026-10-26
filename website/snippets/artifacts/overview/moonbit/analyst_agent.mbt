#derive.agent
#derive.mount("/chats/{chat}")
// Golem serves this agent's files only when complete:
#derive.expose_files("/*", "/out/$1") //@
struct AnalystAgent {
  chat : String
}

fn AnalystAgent::new(chat : String) -> AnalystAgent {
  { chat, }
}

#derive.endpoint(post="/ask")
pub async fn AnalystAgent::ask(
  self : Self, question : String,
) -> String {
  // the model calls this tool when a user asks for a graph:
  let chart = async fn(q : String) -> String { //@
    guard is_quarter(q) else { fail("not a quarter: \{q}") }
    let name = @api.generate_idempotency_key_string()
    let root = @fs.get_root_dir().unwrap()
    root.create_directory_at("out") |> ignore
    @fs.write_bytes(root, "/out/\{name}.png", chart_png(q)).unwrap() //@
    @fs.write_string(root, "/out/\{name}.html", //@
      page_for(q, name)).unwrap() //@
    "/chats/\{self.chat}/\{name}.html"
  }
  run_model_loop(question, chart, max_steps=5,
    tool_description="Chart sales for a quarter and link to it",
  )
}
