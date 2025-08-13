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
    input: String, // path or URL
    #[arg(long)]
    output: Option<String>, // write WIT to file if provided
}

fn read_to_string(input: &str) -> Result<String> {
    if input.starts_with("http://") || input.starts_with("https://") {
        let text = reqwest::blocking::get(input)?.text()?;
        Ok(text)
    } else {
        Ok(fs::read_to_string(input).with_context(|| format!("reading {}", input))?)
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    let text = read_to_string(&args.input)?;
    let wit_text = match args.from {
        SourceKind::Openapi => openapi_to_wit::convert_openapi_to_wit(&text)?.wit_text,
        SourceKind::Protobuf => protobuf_to_wit::convert_protobuf_to_wit(&text)?.wit_text,
    };
    if let Some(path) = args.output.as_ref() {
        fs::write(path, &wit_text).with_context(|| format!("writing {}", path))?;
    } else {
        println!("{}", wit_text);
    }
    Ok(())
} 