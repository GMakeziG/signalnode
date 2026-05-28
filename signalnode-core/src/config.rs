use crate::deliver::SmtpConfig;

pub struct Config {
    pub database_url: String,
    pub smtp: Option<SmtpConfig>,
    pub poll_interval_secs: u64,
    pub checker_poll_interval_secs: u64,
    pub purge_interval_secs: u64,
    pub tcp_check_timeout_ms: u64,
}

impl Config {
    pub fn from_env() -> Self {
        Self::from_provider(|k| std::env::var(k).ok())
    }

    fn from_provider(get: impl Fn(&str) -> Option<String>) -> Self {
        let database_url = get("DATABASE_URL").expect("DATABASE_URL must be set");

        let poll_interval_secs = get("WORKER_POLL_INTERVAL_SECS")
            .map(|v| v.parse::<u64>().expect("WORKER_POLL_INTERVAL_SECS must be a positive integer"))
            .unwrap_or(10);

        let smtp = get("SMTP_HOST").map(|host| SmtpConfig {
            host,
            port: get("SMTP_PORT")
                .map(|v| v.parse::<u16>().expect("SMTP_PORT must be a valid port number"))
                .unwrap_or(587),
            user: get("SMTP_USER").expect("SMTP_USER required when SMTP_HOST is set"),
            pass: get("SMTP_PASS").expect("SMTP_PASS required when SMTP_HOST is set"),
            from: get("SMTP_FROM").expect("SMTP_FROM required when SMTP_HOST is set"),
        });

        let checker_poll_interval_secs = get("CHECKER_POLL_INTERVAL_SECS")
            .map(|v| v.parse::<u64>().expect("CHECKER_POLL_INTERVAL_SECS must be a positive integer"))
            .unwrap_or(30);

        let purge_interval_secs = get("TOKEN_PURGE_INTERVAL_SECS")
            .map(|v| v.parse::<u64>().expect("TOKEN_PURGE_INTERVAL_SECS must be a positive integer"))
            .unwrap_or(3600);

        let tcp_check_timeout_ms = get("TCP_CHECK_TIMEOUT_MS")
            .map(|v| v.parse::<u64>().expect("TCP_CHECK_TIMEOUT_MS must be a positive integer"))
            .unwrap_or(5000);

        Config { database_url, smtp, poll_interval_secs, checker_poll_interval_secs, purge_interval_secs, tcp_check_timeout_ms }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| pairs.iter().find(|(key, _)| *key == k).map(|(_, v)| v.to_string())
    }

    #[test]
    #[should_panic(expected = "DATABASE_URL must be set")]
    fn from_env_panics_without_database_url() {
        Config::from_provider(|_| None);
    }

    #[test]
    fn from_env_uses_poll_interval_default() {
        let cfg = Config::from_provider(vars(&[("DATABASE_URL", "postgres://unused")]));
        assert_eq!(cfg.poll_interval_secs, 10);
    }

    #[test]
    fn from_env_parses_poll_interval() {
        let cfg = Config::from_provider(vars(&[
            ("DATABASE_URL", "postgres://unused"),
            ("WORKER_POLL_INTERVAL_SECS", "30"),
        ]));
        assert_eq!(cfg.poll_interval_secs, 30);
    }

    #[test]
    fn from_env_smtp_none_when_no_host() {
        let cfg = Config::from_provider(vars(&[("DATABASE_URL", "postgres://unused")]));
        assert!(cfg.smtp.is_none());
    }

    #[test]
    fn from_env_uses_checker_interval_default() {
        let cfg = Config::from_provider(vars(&[("DATABASE_URL", "postgres://unused")]));
        assert_eq!(cfg.checker_poll_interval_secs, 30);
    }

    #[test]
    fn from_env_parses_checker_interval() {
        let cfg = Config::from_provider(vars(&[
            ("DATABASE_URL", "postgres://unused"),
            ("CHECKER_POLL_INTERVAL_SECS", "60"),
        ]));
        assert_eq!(cfg.checker_poll_interval_secs, 60);
    }

    #[test]
    fn from_env_uses_purge_interval_default() {
        let cfg = Config::from_provider(vars(&[("DATABASE_URL", "postgres://unused")]));
        assert_eq!(cfg.purge_interval_secs, 3600);
    }

    #[test]
    fn from_env_parses_purge_interval() {
        let cfg = Config::from_provider(vars(&[
            ("DATABASE_URL", "postgres://unused"),
            ("TOKEN_PURGE_INTERVAL_SECS", "7200"),
        ]));
        assert_eq!(cfg.purge_interval_secs, 7200);
    }

    #[test]
    fn from_env_smtp_some_with_all_vars() {
        let cfg = Config::from_provider(vars(&[
            ("DATABASE_URL", "postgres://unused"),
            ("SMTP_HOST", "smtp.example.com"),
            ("SMTP_PORT", "465"),
            ("SMTP_USER", "user@example.com"),
            ("SMTP_PASS", "secret"),
            ("SMTP_FROM", "from@example.com"),
        ]));
        let smtp = cfg.smtp.expect("smtp should be Some");
        assert_eq!(smtp.host, "smtp.example.com");
        assert_eq!(smtp.port, 465);
        assert_eq!(smtp.user, "user@example.com");
        assert_eq!(smtp.pass, "secret");
        assert_eq!(smtp.from, "from@example.com");
    }

    #[test]
    fn from_env_uses_tcp_check_timeout_default() {
        let cfg = Config::from_provider(vars(&[("DATABASE_URL", "postgres://unused")]));
        assert_eq!(cfg.tcp_check_timeout_ms, 5000);
    }

    #[test]
    fn from_env_parses_tcp_check_timeout() {
        let cfg = Config::from_provider(vars(&[
            ("DATABASE_URL", "postgres://unused"),
            ("TCP_CHECK_TIMEOUT_MS", "3000"),
        ]));
        assert_eq!(cfg.tcp_check_timeout_ms, 3000);
    }
}
