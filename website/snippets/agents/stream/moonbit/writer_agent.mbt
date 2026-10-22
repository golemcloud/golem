#derive.agent
#derive.mount("/docs/{doc}")
struct WriterAgent {
  doc : String
}

fn WriterAgent::new(doc : String) -> WriterAgent {
  { doc, }
}

#derive.endpoint(post="/draft")
pub fn WriterAgent::draft(
  self : Self,
  brief : String,
  // clients resume this Durable Stream from an offset:
) -> @schema.AgentStream[String] { //@
  @schema.AgentStream::produce(async fn(out) {
    for section in ["Summary", "Details", "Actions"] {
      // completed model calls are not repeated on recovery:
      let reply = @llm.generate_text(
        model=@llm.openai("gpt-4.1"),
        prompt="\{brief}\nWrite the \{section} section.",
      )
      guard out.write_one(reply) is Accepted else { return } //@
    }
  })
}
