use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "golem")]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Worker {
        #[command(subcommand)]
        command: WorkerCommands,
    },
}

#[derive(Subcommand)]
enum WorkerCommands {
    Files {
        #[command(subcommand)]
        command: WorkerFilesCommands,
    },
}

#[derive(Subcommand)]
enum WorkerFilesCommands {
    List {
        component_id: String,
        worker_name: String,
    },
    Get {
        component_id: String,
        worker_name: String,
        file_name: String,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match cli.command {
        Commands::Worker { command } => match command {
            WorkerCommands::Files { command } => match command {
                WorkerFilesCommands::List {
                    component_id: _,
                    worker_name: _,
                } => {
                    println!("Not implemented yet");
                }
                WorkerFilesCommands::Get {
                    component_id: _,
                    worker_name: _,
                    file_name: _,
                } => {
                    println!("Not implemented yet");
                }
            },
        },
    }
}