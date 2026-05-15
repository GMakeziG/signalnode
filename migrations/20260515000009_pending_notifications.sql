CREATE TABLE pending_notifications (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    incident_id  UUID        NOT NULL REFERENCES incidents(id) ON DELETE CASCADE,
    channel_kind TEXT        NOT NULL CHECK (channel_kind IN ('email', 'webhook')),
    target       TEXT        NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX pending_notifications_incident_id_idx
    ON pending_notifications (incident_id);
CREATE INDEX pending_notifications_created_at_idx
    ON pending_notifications (created_at);
