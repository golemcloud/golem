#[tool_definition]
pub trait Summarize {
  async fn summarize(&self, url: Url) -> Result<String, AppError>;
}
#[tool_implementation]
impl Summarize for SummarizeImpl {
  // each call gets a fresh VM that can't read the agent's memory:
  async fn summarize(&self, url: Url) -> Result<String, AppError> { //@
    let req = Request::get(url).body(vec![])?;
    // recovery reuses this completed fetch's response:
    let page = http::response_text(http::send(req).await?).await?; //@
    Ok(summarize_text(&page).await?)
  }
}

#[agent_definition]
pub trait DigestAgent {
  fn new(topic: String) -> Self;
  async fn build(&self, urls: Urls) -> Result<Urls, AppError>;
}
#[agent_implementation]
impl DigestAgent for DigestAgentImpl {
  fn new(_topic: String) -> Self { Self }
  async fn build(&self, urls: Urls) -> Result<Urls, AppError> {
    let mut out = vec![];
    for url in urls {
      out.push(SummarizeClient::new().summarize(url).await?);
    }
    Ok(out)
  }
}
