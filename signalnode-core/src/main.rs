use std::time::Duration;
use tracing::info;
use tracing_subscriber::EnvFilter;

mod config;
mod deliver;
mod worker;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .json()
        .init();

    info!("signalnode-core starting");

    let cfg = config::Config::from_env();

    let pool = sqlx::PgPool::connect(&cfg.database_url)
        .await
        .expect("failed to connect to database");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("failed to build HTTP client");

    let interval = Duration::from_secs(cfg.poll_interval_secs);

    info!(
        interval_secs = cfg.poll_interval_secs,
        smtp_configured = cfg.smtp.is_some(),
        "starting notification delivery worker"
    );

    worker::run_worker(pool, client, cfg.smtp, interval).await;
}
