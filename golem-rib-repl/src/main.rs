mod dependency_manager;
mod history;
mod result_printer;
mod rib_repl;
mod syntax_highlighter;

#[tokio::main]
async fn main() {
    let mut repl = RibRepl::new();
    repl.run().await;
}
