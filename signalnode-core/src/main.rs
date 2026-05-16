use tracing::info;
use tracing_subscriber::EnvFilter;

mod config;
mod deliver;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .json()
        .init();

    info!("signalnode-core starting");
}
