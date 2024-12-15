use clap::Parser;
use golem_ui_service::CliArgs;
use golem_ui_service::UiService;
#[tokio::main]
pub async fn main() {
    let args = CliArgs::parse();
    
    let res = UiService::new(args)
        .run()
        .await;
        
    match res {
        Err(e) => println!("Error starting UI: {}", e),
        Ok(_) => println!("UI Server stopped"),
    }
}