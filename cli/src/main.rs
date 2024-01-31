extern crate derive_more;

use std::fmt::Debug;

use clap::{Parser, Subcommand};
use clap_verbosity_flag::{Level, Verbosity};
use golem_client::Context;
use golem_examples::model::{ExampleName, GuestLanguage, GuestLanguageTier, PackageName};
use model::*;
use reqwest::Url;
use tracing_subscriber::FmtSubscriber;

use crate::clients::template::TemplateClientLive;
use crate::clients::worker::WorkerClientLive;
use crate::template::{TemplateHandler, TemplateHandlerLive, TemplateSubcommand};
use crate::worker::{WorkerHandler, WorkerHandlerLive, WorkerSubcommand};

pub mod clients;
mod examples;
pub mod model;
mod template;
mod worker;

pub fn parse_key_val(
    s: &str,
) -> Result<(String, String), Box<dyn std::error::Error + Send + Sync + 'static>> {
    let pos = s
        .find('=')
        .ok_or_else(|| format!("invalid KEY=value: no `=` found in `{s}`"))?;
    Ok((s[..pos].parse()?, s[pos + 1..].parse()?))
}

#[derive(Subcommand, Debug)]
#[command()]
enum Command {
    #[command()]
    Template {
        #[command(subcommand)]
        subcommand: TemplateSubcommand,
    },
    #[command()]
    Worker {
        #[command(subcommand)]
        subcommand: WorkerSubcommand,
    },
    #[command()]
    New {
        #[arg(short, long)]
        example: ExampleName,

        #[arg(short, long)]
        template_name: golem_examples::model::TemplateName,

        #[arg(short, long)]
        package_name: Option<PackageName>,
    },
    #[command()]
    ListExamples {
        #[arg(short, long)]
        min_tier: Option<GuestLanguageTier>,

        #[arg(short, long)]
        language: Option<GuestLanguage>,
    },
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about, rename_all = "kebab-case")]
/// Command line interface for OSS version of Golem.
///
/// For Golem Cloud client see golem-cloud-cli instead: https://github.com/golemcloud/golem-cloud-cli
struct GolemCommand {
    #[command(flatten)]
    verbosity: Verbosity,

    #[arg(short = 'F', long, default_value = "yaml")]
    format: Format,

    #[arg(short = 'u', long)]
    /// Golem base url. Default: GOLEM_BASE_URL environment variable or http://localhost:9881.
    golem_url: Option<String>,

    #[command(subcommand)]
    command: Command,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let command = GolemCommand::parse();

    if let Some(level) = command.verbosity.log_level() {
        let tracing_level = match level {
            Level::Error => tracing::Level::ERROR,
            Level::Warn => tracing::Level::WARN,
            Level::Info => tracing::Level::INFO,
            Level::Debug => tracing::Level::DEBUG,
            Level::Trace => tracing::Level::TRACE,
        };

        let subscriber = FmtSubscriber::builder()
            .with_max_level(tracing_level)
            .with_writer(std::io::stderr)
            .finish();

        tracing::subscriber::set_global_default(subscriber)
            .expect("setting default subscriber failed");
    }

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async_main(command))
}

async fn async_main(cmd: GolemCommand) -> Result<(), Box<dyn std::error::Error>> {
    let url_str = cmd
        .golem_url
        .or_else(|| std::env::var("GOLEM_BASE_URL").ok())
        .unwrap_or("http://localhost:9881".to_string());
    let url = Url::parse(&url_str).unwrap();
    let allow_insecure_str = std::env::var("GOLEM_ALLOW_INSECURE").unwrap_or("false".to_string());
    let allow_insecure = allow_insecure_str != "false";

    let mut builder = reqwest::Client::builder();
    if allow_insecure {
        builder = builder.danger_accept_invalid_certs(true);
    }
    let client = builder.connection_verbose(true).build()?;

    let context = Context {
        base_url: url.clone(),
        client: client.clone(),
    };

    let template_client = TemplateClientLive {
        client: golem_client::api::TemplateClientLive {
            context: context.clone(),
        },
    };
    let template_srv = TemplateHandlerLive {
        client: template_client,
    };
    let worker_client = WorkerClientLive {
        client: golem_client::api::WorkerClientLive {
            context: context.clone(),
        },
        context: context.clone(),
        allow_insecure,
    };
    let worker_srv = WorkerHandlerLive {
        client: worker_client,
        templates: &template_srv,
    };

    let res = match cmd.command {
        Command::Template { subcommand } => template_srv.handle(subcommand).await,
        Command::Worker { subcommand } => worker_srv.handle(subcommand).await,
        Command::New {
            example,
            package_name,
            template_name,
        } => examples::process_new(example, template_name, package_name),
        Command::ListExamples { min_tier, language } => {
            examples::process_list_examples(min_tier, language)
        }
    };

    match res {
        Ok(res) => match res {
            GolemResult::Ok(r) => {
                r.println(&cmd.format);

                Ok(())
            }
            GolemResult::Str(s) => {
                println!("{s}");

                Ok(())
            }
            GolemResult::Json(json) => match &cmd.format {
                Format::Json => Ok(println!("{}", serde_json::to_string_pretty(&json).unwrap())),
                Format::Yaml => Ok(println!("{}", serde_yaml::to_string(&json).unwrap())),
            },
        },
        Err(err) => Err(Box::new(err)),
    }
}
