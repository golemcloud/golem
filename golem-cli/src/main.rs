use clap::{Parser, Subcommand};
use golem_client::worker_files::WorkerFilesClient;
use tokio::io::{AsyncWriteExt, stdout};

#[derive(Parser)]
#[command(name = "golem")]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[arg(long, env = "GOLEM_URL", default_value = "http://localhost:8080")]
    url: String,
    #[arg(long, env = "GOLEM_TOKEN")]
    token: Option<String>,
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
        #[arg(long)]
        file: Option<String>,
        #[arg(long)]
        json: bool,
    },
    Get {
        component_id: String,
        worker_name: String,
        file_name: String,
        #[arg(long)]
        output: Option<String>,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let client = WorkerFilesClient::new(cli.url, cli.token);
    let result = match cli.command {
        Commands::Worker { command } => match command {
            WorkerCommands::Files { command } => match command {
                WorkerFilesCommands::List { component_id, worker_name, file, json } => {
                    let file_name = file.unwrap_or_default();
                    match client.list_files(&component_id, &worker_name, &file_name).await {
                        Ok(resp) => {
                            if json {
                                println!("{}", serde_json::to_string_pretty(&resp).unwrap());
                            } else {
                                for n in resp.nodes {
                                    let kind = if n.is_dir { "dir" } else { "file" };
                                    println!("{}\t{}\t{}", kind, n.path, n.name);
                                }
                            }
                            Ok(())
                        }
                        Err(e) => Err(e),
                    }
                }
                WorkerFilesCommands::Get { component_id, worker_name, file_name, output } => {
                    match client.get_file_content(&component_id, &worker_name, &file_name).await {
                        Ok(bytes) => {
                            if let Some(path) = output {
                                tokio::fs::write(path, &bytes).await.unwrap();
                            } else {
                                let mut out = stdout();
                                out.write_all(&bytes).await.unwrap();
                            }
                            Ok(())
                        }
                        Err(e) => Err(e),
                    }
                }
            },
        },
    };
    if let Err(err) = result {
        eprintln!("Error: {err}");
        std::process::exit(1);
    }
}