CREATE TABLE check_results (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    monitor_id   UUID        NOT NULL REFERENCES monitors(id) ON DELETE CASCADE,
    status       TEXT        NOT NULL CHECK (status IN ('up', 'degraded', 'down')),
    latency_ms   INT,
    error_detail TEXT,
    checked_at   TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX check_results_monitor_id_idx ON check_results (monitor_id, checked_at DESC);
