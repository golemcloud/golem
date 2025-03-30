mod history;
mod syntax_highlighter;
mod dependency_manager;
mod rib_repl;


#[tokio::main]
async fn main() {
    let mut repl = RibRepl::new();
    repl.run().await;
}
