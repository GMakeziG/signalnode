use std::time::Duration;
use tracing::info;
use tracing_subscriber::EnvFilter;

mod checker;
mod config;
mod deliver;
mod purger;
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

    let worker_interval = Duration::from_secs(cfg.poll_interval_secs);
    let checker_interval = Duration::from_secs(cfg.checker_poll_interval_secs);

    info!(
        worker_interval_secs = cfg.poll_interval_secs,
        checker_interval_secs = cfg.checker_poll_interval_secs,
        smtp_configured = cfg.smtp.is_some(),
        "signalnode-core starting workers"
    );

    let h1 = tokio::spawn(worker::run_worker(
        pool.clone(),
        client.clone(),
        cfg.smtp,
        worker_interval,
    ));
    let h2 = tokio::spawn(checker::run_checker(pool, client, checker_interval));
    let (r1, r2) = tokio::join!(h1, h2);
    r1.expect("delivery worker panicked");
    r2.expect("checker panicked");
}
