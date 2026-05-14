CREATE TABLE monitors (
    id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id  UUID        NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    name          TEXT        NOT NULL,
    url           TEXT        NOT NULL,
    interval_secs INT         NOT NULL CHECK (interval_secs > 0),
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX monitors_workspace_id_idx ON monitors (workspace_id);
