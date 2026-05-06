-- Replace IMSI component columns with a single full IMSI string.
-- The BSC always resolves the full 15-digit IMSI (MCC + IMSI_11_12 + IMSI_S)
-- before upserting, so we no longer need the decomposed fields.

ALTER TABLE mobiles_seen
    ADD COLUMN IF NOT EXISTS imsi TEXT;

-- Backfill: best-effort reconstruction is not possible without overhead
-- context, so existing rows keep NULL imsi and will be updated on next sighting.

ALTER TABLE mobiles_seen
    DROP COLUMN IF EXISTS imsi_m_s1,
    DROP COLUMN IF EXISTS imsi_m_s2,
    DROP COLUMN IF EXISTS imsi_mcc,
    DROP COLUMN IF EXISTS imsi_11_12,
    DROP COLUMN IF EXISTS imsi_class;

-- Replace the old component-based unique index with a simple IMSI string index.
DROP INDEX IF EXISTS idx_mobiles_seen_imsi;
CREATE UNIQUE INDEX idx_mobiles_seen_imsi ON mobiles_seen (imsi) WHERE imsi IS NOT NULL;
