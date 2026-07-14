use clap::Parser;

use aeon_memory_gateway::cli::Cli;
use aeon_memory_gateway::{config::discover_and_load_config, runtime::build_core};

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let result = async {
        let (config_path, config) =
            discover_and_load_config(cli.config.as_deref()).map_err(|error| error.to_string())?;
        eprintln!(
            "aeon-memory: config={}, data={}",
            config_path.display(),
            config.data.base_dir
        );
        let service = build_core(&config)
            .await
            .map_err(|error| error.to_string())?;
        let command = aeon_memory_gateway::cli::execute(service.clone(), cli)
            .await
            .map_err(|error| error.to_string());
        let shutdown = service.shutdown().await.map_err(|error| error.to_string());
        match (command, shutdown) {
            (Ok(output), Ok(())) => Ok(output),
            (Err(command), Ok(())) => Err(command),
            (Ok(_), Err(shutdown)) => Err(format!("core shutdown failed: {shutdown}")),
            (Err(command), Err(shutdown)) => Err(format!(
                "{command}; additionally core shutdown failed: {shutdown}"
            )),
        }
    }
    .await;
    match result {
        Ok(output) => println!("{output}"),
        Err(error) => {
            eprintln!("aeon-memory: {error}");
            std::process::exit(1);
        }
    }
}
