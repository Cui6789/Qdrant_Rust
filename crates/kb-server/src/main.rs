use kb_core::config::AppConfig;

fn main() -> anyhow::Result<()> {
    let config = AppConfig::load(None)?;
    println!(
        "kb-server configured for {}:{}",
        config.server.host, config.server.port
    );
    Ok(())
}
