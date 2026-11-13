use rusqlite::Connection;
#[agent_definition]
pub trait NotesAgent {
  fn new(user: String) -> Self;
  fn add(&mut self, text: String) -> Result<(), AppError>;
  fn find(&self, term: String) -> Result<Vec<String>, AppError>;
}
fn open() -> Result<Connection, AppError> {
  fs::create_dir_all("/data")?;
  // SQLite uses this agent's durable file system, not a server:
  let db = Connection::open("/data/notes.db")?; //@
  db.execute_batch(
    "CREATE TABLE IF NOT EXISTS notes (text TEXT NOT NULL)")?;
  Ok(db)
}
struct NotesAgentImpl;
#[agent_implementation]
impl NotesAgent for NotesAgentImpl {
  fn new(_user: String) -> Self { Self }
  fn add(&mut self, text: String) -> Result<(), AppError> {
    open()?.execute("INSERT INTO notes VALUES (?)", [text])?;
    Ok(())
  }
  fn find(&self, term: String) -> Result<Vec<String>, AppError> {
    let db = open()?;
    let mut stmt =
      db.prepare("SELECT text FROM notes WHERE text LIKE ?")?;
    let rows = stmt.query_map([format!("%{term}%")], |r| r.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
  }
}
