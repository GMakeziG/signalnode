-- migrations/20260518000013_login_attempts.sql
CREATE TABLE login_attempts (
    user_id        UUID        PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    failed_count   INT         NOT NULL DEFAULT 0,
    locked_until   TIMESTAMPTZ,
    last_failed_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
