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
            .map(|v| v.parse::<u64>().expect("WORKER_POLL_INTERVAL_SECS must be a positive integer"))
            .unwrap_or(10);

        let smtp = std::env::var("SMTP_HOST").ok().map(|host| SmtpConfig {
            host,
            port: std::env::var("SMTP_PORT")
                .ok()
                .map(|v| v.parse::<u16>().expect("SMTP_PORT must be a valid port number"))
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
    use serial_test::serial;

    #[test]
    #[serial]
    #[should_panic(expected = "DATABASE_URL must be set")]
    fn from_env_panics_without_database_url() {
        std::env::remove_var("DATABASE_URL");
        std::env::remove_var("SMTP_HOST");
        Config::from_env();
    }

    #[test]
    #[serial]
    fn from_env_uses_poll_interval_default() {
        std::env::set_var("DATABASE_URL", "postgres://unused");
        std::env::remove_var("SMTP_HOST");
        std::env::remove_var("WORKER_POLL_INTERVAL_SECS");
        let cfg = Config::from_env();
        assert_eq!(cfg.poll_interval_secs, 10);
        std::env::remove_var("DATABASE_URL");
    }

    #[test]
    #[serial]
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
    #[serial]
    fn from_env_smtp_none_when_no_host() {
        std::env::set_var("DATABASE_URL", "postgres://unused");
        std::env::remove_var("SMTP_HOST");
        let cfg = Config::from_env();
        assert!(cfg.smtp.is_none());
        std::env::remove_var("DATABASE_URL");
    }

    #[test]
    #[serial]
    fn from_env_smtp_some_with_all_vars() {
        std::env::set_var("DATABASE_URL", "postgres://unused");
        std::env::set_var("SMTP_HOST", "smtp.example.com");
        std::env::set_var("SMTP_PORT", "465");
        std::env::set_var("SMTP_USER", "user@example.com");
        std::env::set_var("SMTP_PASS", "secret");
        std::env::set_var("SMTP_FROM", "from@example.com");
        let cfg = Config::from_env();
        let smtp = cfg.smtp.expect("smtp should be Some");
        assert_eq!(smtp.host, "smtp.example.com");
        assert_eq!(smtp.port, 465);
        assert_eq!(smtp.user, "user@example.com");
        assert_eq!(smtp.pass, "secret");
        assert_eq!(smtp.from, "from@example.com");
        std::env::remove_var("DATABASE_URL");
        std::env::remove_var("SMTP_HOST");
        std::env::remove_var("SMTP_PORT");
        std::env::remove_var("SMTP_USER");
        std::env::remove_var("SMTP_PASS");
        std::env::remove_var("SMTP_FROM");
    }
}
