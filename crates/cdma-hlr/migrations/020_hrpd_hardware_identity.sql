-- Optional 15-digit IMEI per handset identity. Multimode handsets report the
-- IMEI's first 14 digits as the HRPD Hardware ID, which can differ from the
-- MEID they report on 1x.
ALTER TABLE subscriber_identities
    ADD COLUMN IF NOT EXISTS imei TEXT;

CREATE UNIQUE INDEX IF NOT EXISTS idx_identity_imei
    ON subscriber_identities (imei) WHERE imei IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_identity_imei_meid_form
    ON subscriber_identities (left(imei, 14)) WHERE imei IS NOT NULL;

-- An HRPD access network knows a handset only by its Hardware ID, so its
-- admission records have no IMSI.
CREATE UNIQUE INDEX IF NOT EXISTS idx_mobiles_seen_hardware_meid
    ON mobiles_seen (meid) WHERE imsi IS NULL AND meid IS NOT NULL;

CREATE UNIQUE INDEX IF NOT EXISTS idx_mobiles_seen_hardware_esn
    ON mobiles_seen (esn) WHERE imsi IS NULL AND meid IS NULL AND esn IS NOT NULL;
