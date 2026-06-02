-- Preferred Roaming Lists for the OTASP `*228` flow. raw_bytes is the
-- canonical on-wire PRL; pr_list_id / sspr_p_rev are cached for filtering.
CREATE TABLE IF NOT EXISTS prls (
    prl_id        UUID PRIMARY KEY,
    name          VARCHAR(120) NOT NULL,
    pr_list_id    INT NOT NULL,
    sspr_p_rev    SMALLINT NOT NULL,
    is_default    BOOLEAN NOT NULL DEFAULT FALSE,
    raw_bytes     BYTEA NOT NULL,
    notes         TEXT NOT NULL DEFAULT '',
    deleted_at    TIMESTAMPTZ NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Name unique among active rows so soft-deletion frees the label.
CREATE UNIQUE INDEX IF NOT EXISTS idx_prls_name_active
    ON prls (name) WHERE deleted_at IS NULL;

-- At most one default among active rows.
CREATE UNIQUE INDEX IF NOT EXISTS idx_prls_only_one_default
    ON prls (is_default) WHERE is_default = TRUE AND deleted_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_prls_pr_list_id
    ON prls (pr_list_id) WHERE deleted_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_prls_sspr_p_rev
    ON prls (sspr_p_rev) WHERE deleted_at IS NULL;

-- Per-subscriber override of the system-default PRL. RESTRICT on the
-- FK; soft delete is blocked at the application layer when referenced.
ALTER TABLE subscribers
    ADD COLUMN prl_override_id UUID NULL REFERENCES prls(prl_id);

CREATE INDEX IF NOT EXISTS idx_subscribers_prl_override
    ON subscribers (prl_override_id) WHERE prl_override_id IS NOT NULL;
