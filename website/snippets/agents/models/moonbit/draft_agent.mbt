#derive.golem_schema
struct Review { ok : Bool; notes : String } derive(FromJson)
#derive.golem_schema
struct Draft { text : String; review : Review }

#derive.agent
struct DraftAgent {
  topic : String
}

fn DraftAgent::new(topic : String) -> DraftAgent {
  { topic, }
}

pub async fn DraftAgent::write(
  self : Self, idea : String,
) -> Draft {
  // draft with one provider, review with another:
  let draft = @llm.generate_text(
    model=@llm.openai("gpt-4.1"), //@
    prompt="Product update on \{self.topic}: \{idea}",
  )
  let review : Review = @llm.generate_object(
    model=@llm.anthropic("claude-sonnet-4-5"), //@
    prompt="Review this product update:\n\{draft}",
  )
  { text: draft, review }
}
