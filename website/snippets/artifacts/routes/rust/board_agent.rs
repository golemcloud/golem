const BOARD_PAGE: &str = r#"<button id="add">Add</button>
<script>add.onclick = () => fetch('cards', {
  method: 'POST', body: JSON.stringify({ title: 'New card' }),
})</script>"#;

#[agent_definition(
  // the URL names the agent: one BoardAgent per team:
  mount = "/boards/{team}", //@
  filesystem_bindings = [("/app", "/index.html")])]
pub trait BoardAgent {
  fn new(team: String) -> Self;
  // the page's Add button posts to this endpoint:
  #[endpoint(post = "/cards")] //@
  fn add(&mut self, title: String) -> Vec<String>;
}
struct BoardAgentImpl { cards: Vec<String> }
#[agent_implementation]
impl BoardAgent for BoardAgentImpl {
  fn new(_team: String) -> Self {
    fs::write("/index.html", BOARD_PAGE)
      .expect("cannot write the page");
    Self { cards: vec![] }
  }
  fn add(&mut self, title: String) -> Vec<String> {
    self.cards.push(title);
    self.cards.clone()
  }
}
