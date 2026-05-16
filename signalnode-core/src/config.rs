pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub pass: String,
    pub from: String,
}

pub struct Config {
    pub database_url: String,
    pub smtp: Option<SmtpConfig>,
    pub poll_interval_secs: u64,
}

impl Config {
    pub fn from_env() -> Self {
        let database_url = std::env::var("DATABASE_URL")
            .expect("DATABASE_URL must be set");

        let poll_interval_secs = std::env::var("WORKER_POLL_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10);

        let smtp = std::env::var("SMTP_HOST").ok().map(|host| SmtpConfig {
            host,
            port: std::env::var("SMTP_PORT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(587),
            user: std::env::var("SMTP_USER")
                .expect("SMTP_USER required when SMTP_HOST is set"),
            pass: std::env::var("SMTP_PASS")
                .expect("SMTP_PASS required when SMTP_HOST is set"),
            from: std::env::var("SMTP_FROM")
                .expect("SMTP_FROM required when SMTP_HOST is set"),
        });

        Config { database_url, smtp, poll_interval_secs }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic(expected = "DATABASE_URL must be set")]
    fn from_env_panics_without_database_url() {
        std::env::remove_var("DATABASE_URL");
        std::env::remove_var("SMTP_HOST");
        Config::from_env();
    }

    #[test]
    fn from_env_uses_poll_interval_default() {
        std::env::set_var("DATABASE_URL", "postgres://unused");
        std::env::remove_var("SMTP_HOST");
        std::env::remove_var("WORKER_POLL_INTERVAL_SECS");
        let cfg = Config::from_env();
        assert_eq!(cfg.poll_interval_secs, 10);
        std::env::remove_var("DATABASE_URL");
    }

    #[test]
    fn from_env_parses_poll_interval() {
        std::env::set_var("DATABASE_URL", "postgres://unused");
        std::env::set_var("WORKER_POLL_INTERVAL_SECS", "30");
        std::env::remove_var("SMTP_HOST");
        let cfg = Config::from_env();
        assert_eq!(cfg.poll_interval_secs, 30);
        std::env::remove_var("DATABASE_URL");
        std::env::remove_var("WORKER_POLL_INTERVAL_SECS");
    }

    #[test]
    fn from_env_smtp_none_when_no_host() {
        std::env::set_var("DATABASE_URL", "postgres://unused");
        std::env::remove_var("SMTP_HOST");
        let cfg = Config::from_env();
        assert!(cfg.smtp.is_none());
        std::env::remove_var("DATABASE_URL");
    }
}
