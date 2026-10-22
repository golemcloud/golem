#derive.tool("summarize")
struct Summarize {}

// each call gets a fresh VM that can't read the agent's memory:
pub async fn Summarize::summarize(url : String) -> String { //@
  // recovery reuses this completed fetch's response:
  let page = read_text(@http.send(get(url)).unwrap()) //@
  summarize_text(page)
}

#derive.agent
struct DigestAgent {
  topic : String
}

fn DigestAgent::new(topic : String) -> DigestAgent {
  { topic, }
}

pub async fn DigestAgent::build(
  self : Self, urls : Array[String],
) -> Array[String] {
  let out = []
  for url in urls {
    out.push(SummarizeClient::new().summarize(url).unwrap())
  }
  out
}
