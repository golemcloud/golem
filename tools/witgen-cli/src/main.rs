use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use std::fs;

#[derive(Copy, Clone, Eq, PartialEq, ValueEnum, Debug)]
enum SourceKind { Openapi, Protobuf }

#[derive(Parser, Debug)]
#[command(name = "witgen-cli", about = "Preview WIT generation from OpenAPI or Protobuf")] 
struct Args {
    #[arg(long, value_enum)]
    from: SourceKind,
    #[arg(long)]
    input: String,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let text = fs::read_to_string(&args.input).with_context(|| format!("reading {}", args.input))?;
    match args.from {
        SourceKind::Openapi => {
            let out = openapi_to_wit::convert_openapi_to_wit(&text)?;
            println!("{}", out.wit_text);
        }
        SourceKind::Protobuf => {
            let out = protobuf_to_wit::convert_protobuf_to_wit(&text)?;
            println!("{}", out.wit_text);
        }
    }
    Ok(())
} 