-- migrations/20260518000012_refresh_tokens.sql
CREATE TABLE refresh_tokens (
    jti        UUID        PRIMARY KEY,
    user_id    UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX refresh_tokens_user_id_idx ON refresh_tokens (user_id);

-- NOTE: Expired rows (expires_at < NOW()) are not automatically removed.
-- A periodic cleanup job (DELETE FROM refresh_tokens WHERE expires_at < NOW())
-- should be scheduled before this table reaches significant volume.
