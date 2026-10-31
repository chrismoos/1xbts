-- Latest MSC admission decision per observed mobile, and its denial history.
-- The latest decision replaces the admission_* columns. A denial also
-- increments denied_count and replaces the last_denied_* columns, which a
-- later admitted decision leaves in place.
ALTER TABLE mobiles_seen
    ADD COLUMN admission_class TEXT NULL,
    ADD COLUMN admission_reason TEXT NULL,
    ADD COLUMN admission_at TIMESTAMPTZ NULL,
    ADD COLUMN admission_base_station TEXT NULL,
    ADD COLUMN denied_count INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN last_denied_at TIMESTAMPTZ NULL,
    ADD COLUMN last_denial_reason TEXT NULL,
    ADD COLUMN last_denied_base_station TEXT NULL;

ALTER TABLE mobiles_seen
    ADD CONSTRAINT chk_mobiles_seen_admission_class
        CHECK (admission_class IS NULL OR admission_class IN
            ('admitted', 'limited', 'provisioning', 'registration_only', 'denied'));

CREATE INDEX IF NOT EXISTS idx_mobiles_seen_last_seen ON mobiles_seen (last_seen_at DESC);
