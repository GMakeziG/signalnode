CREATE TABLE incidents (
    id         UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    monitor_id UUID        NOT NULL REFERENCES monitors(id) ON DELETE CASCADE,
    opened_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    closed_at  TIMESTAMPTZ
);

CREATE INDEX incidents_monitor_id_idx ON incidents (monitor_id, opened_at DESC);
CREATE INDEX incidents_open_idx       ON incidents (monitor_id) WHERE closed_at IS NULL;
