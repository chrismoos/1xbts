ALTER TABLE registration_bindings
    ADD COLUMN IF NOT EXISTS imsi VARCHAR(15),
    ADD COLUMN IF NOT EXISTS esn BIGINT;

UPDATE registration_bindings
SET imsi = COALESCE(imsi, canonical_imsi),
    esn = COALESCE(esn, fwd_esn, page_esn);

DROP INDEX IF EXISTS idx_identity_imsi;
CREATE UNIQUE INDEX IF NOT EXISTS idx_identity_imsi
    ON subscriber_identities(imsi)
    WHERE imsi IS NOT NULL;

ALTER TABLE subscriber_identities
    DROP COLUMN IF EXISTS imsi_m_s1,
    DROP COLUMN IF EXISTS imsi_m_s2,
    DROP COLUMN IF EXISTS imsi_mcc,
    DROP COLUMN IF EXISTS imsi_11_12,
    DROP COLUMN IF EXISTS imsi_class;

ALTER TABLE registration_bindings
    DROP COLUMN IF EXISTS canonical_imsi,
    DROP COLUMN IF EXISTS fwd_esn,
    DROP COLUMN IF EXISTS fwd_imsi_m_s1,
    DROP COLUMN IF EXISTS fwd_imsi_m_s2,
    DROP COLUMN IF EXISTS page_esn,
    DROP COLUMN IF EXISTS page_imsi_m_s1,
    DROP COLUMN IF EXISTS page_imsi_m_s2,
    DROP COLUMN IF EXISTS page_mcc,
    DROP COLUMN IF EXISTS page_imsi_11_12;
