CREATE TABLE notification_channels (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID        NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    kind         TEXT        NOT NULL CHECK (kind IN ('email', 'webhook')),
    target       TEXT        NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX notification_channels_workspace_id_idx
    ON notification_channels (workspace_id);
