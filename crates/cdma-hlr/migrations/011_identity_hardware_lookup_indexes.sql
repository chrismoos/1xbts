-- Supports ESN-only / MEID-only subscriber resolution for OTASP `*228`
-- flows, where the MS has hardware IDs but no IMSI yet.
CREATE INDEX IF NOT EXISTS idx_identity_esn_lookup
    ON subscriber_identities(esn)
    WHERE esn IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_identity_meid_lookup
    ON subscriber_identities(meid)
    WHERE meid IS NOT NULL;
