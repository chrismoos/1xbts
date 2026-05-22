ALTER TABLE subscriber_identities
    ADD COLUMN IF NOT EXISTS meid TEXT;

ALTER TABLE registration_bindings
    ADD COLUMN IF NOT EXISTS meid TEXT;

ALTER TABLE mobiles_seen
    ADD COLUMN IF NOT EXISTS meid TEXT;

DROP INDEX IF EXISTS idx_identity_esn;
DROP INDEX IF EXISTS idx_identity_imsi;

ALTER TABLE subscriber_identities
    DROP CONSTRAINT IF EXISTS chk_subscriber_identity_complete;

ALTER TABLE subscriber_identities
    ADD CONSTRAINT chk_subscriber_identity_complete
    CHECK (imsi IS NOT NULL AND (esn IS NOT NULL OR meid IS NOT NULL))
    NOT VALID;

CREATE UNIQUE INDEX IF NOT EXISTS idx_identity_imsi_esn
    ON subscriber_identities(imsi, esn)
    WHERE imsi IS NOT NULL AND esn IS NOT NULL AND meid IS NULL;

CREATE UNIQUE INDEX IF NOT EXISTS idx_identity_imsi_meid
    ON subscriber_identities(imsi, meid)
    WHERE imsi IS NOT NULL AND esn IS NULL AND meid IS NOT NULL;

CREATE UNIQUE INDEX IF NOT EXISTS idx_identity_imsi_esn_meid
    ON subscriber_identities(imsi, esn, meid)
    WHERE imsi IS NOT NULL AND esn IS NOT NULL AND meid IS NOT NULL;

DROP INDEX IF EXISTS idx_mobiles_seen_esn;
DROP INDEX IF EXISTS idx_mobiles_seen_imsi;

CREATE UNIQUE INDEX IF NOT EXISTS idx_mobiles_seen_imsi_esn
    ON mobiles_seen(imsi, esn)
    WHERE imsi IS NOT NULL AND esn IS NOT NULL AND meid IS NULL;

CREATE UNIQUE INDEX IF NOT EXISTS idx_mobiles_seen_imsi_meid
    ON mobiles_seen(imsi, meid)
    WHERE imsi IS NOT NULL AND esn IS NULL AND meid IS NOT NULL;

CREATE UNIQUE INDEX IF NOT EXISTS idx_mobiles_seen_imsi_esn_meid
    ON mobiles_seen(imsi, esn, meid)
    WHERE imsi IS NOT NULL AND esn IS NOT NULL AND meid IS NOT NULL;

CREATE UNIQUE INDEX IF NOT EXISTS idx_mobiles_seen_legacy_imsi
    ON mobiles_seen(imsi)
    WHERE imsi IS NOT NULL AND esn IS NULL AND meid IS NULL;
