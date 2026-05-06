ALTER TABLE subscriber_identities
    ADD COLUMN IF NOT EXISTS imsi VARCHAR(15);

CREATE UNIQUE INDEX IF NOT EXISTS idx_identity_canonical_imsi
    ON subscriber_identities(imsi)
    WHERE imsi IS NOT NULL;
