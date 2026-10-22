use golem_ai_llm::LlmProvider;
use golem_ai_llm_anthropic::{Anthropic, AnthropicConfig};
use golem_ai_llm_openai::{OpenAI, OpenAiConfig};
#[derive(Schema)] pub struct Review { ok: bool, notes: String }
#[derive(Schema)] pub struct Draft { text: String, review: Review }

#[agent_definition]
pub trait DraftAgent {
  fn new(topic: String) -> Self;
  async fn write(&self, idea: String) -> Result<Draft, AppError>;
}
struct DraftAgentImpl { topic: String }
#[agent_implementation]
impl DraftAgent for DraftAgentImpl {
  fn new(topic: String) -> Self { Self { topic } }
  async fn write(&self, idea: String) -> Result<Draft, AppError> {
    // completed model calls are not repeated on recovery:
    let draft = text(OpenAI::send( //@
      OpenAiConfig::from_env()?,
      vec![user(&format!("Product update on {}: {idea}",
        self.topic))],
      model("gpt-4.1"),
    ).await)?;
    let review = json(Anthropic::send( //@
      AnthropicConfig::from_env()?,
      vec![user(&format!("Review this product update:\n{draft}"))],
      json_model::<Review>("claude-sonnet-4-5"),
    ).await)?;
    Ok(Draft { text: draft, review })
  }
}
