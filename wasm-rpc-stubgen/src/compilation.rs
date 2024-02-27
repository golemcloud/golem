use cargo_component::config::{CargoArguments, Config};
use cargo_component::{load_component_metadata, load_metadata, run_cargo_command};
use cargo_component_core::terminal::{Color, Terminal, Verbosity};
use std::path::Path;

pub async fn compile(root: &Path) -> anyhow::Result<()> {
    let current_dir = std::env::current_dir()?;
    std::env::set_current_dir(root)?;

    let cargo_args = CargoArguments {
        release: true,
        manifest_path: Some(root.join("Cargo.toml")),
        ..Default::default()
    };

    let config = Config::new(Terminal::new(Verbosity::Normal, Color::Auto))?;

    let metadata = load_metadata(cargo_args.manifest_path.as_deref())?;
    let packages =
        load_component_metadata(&metadata, cargo_args.packages.iter(), cargo_args.workspace)?;

    run_cargo_command(
        &config,
        &metadata,
        &packages,
        Some("build"),
        &cargo_args,
        &["build".to_string(), "--release".to_string()],
    )
    .await?;

    std::env::set_current_dir(current_dir)?;
    Ok(())
}
