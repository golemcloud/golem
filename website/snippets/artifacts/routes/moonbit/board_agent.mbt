let board_page : String =
  #|<button id="add">Add</button>
  #|<script>add.onclick = () => fetch('cards', {
  #|  method: 'POST', body: JSON.stringify({ title: 'New card' }),
  #|})</script>

#derive.agent
// the URL names the agent: one BoardAgent per team:
#derive.mount("/boards/{team}") //@
#derive.expose_files("/app", "/index.html")
struct BoardAgent {
  team : String
  cards : Array[String]
}

async fn BoardAgent::new(team : String) -> BoardAgent {
  let root = @fs.get_root_dir().unwrap()
  @fs.write_string(root, "/index.html", board_page).unwrap()
  { team, cards: [] }
}

// the page's Add button posts to this endpoint:
#derive.endpoint(post="/cards") //@
pub fn BoardAgent::add(
  self : Self, title : String,
) -> Array[String] {
  self.cards.push(title)
  self.cards
}
