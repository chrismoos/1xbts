-- A-key and SSD generated during an OTASP session. They replace a_key, ssd_a
-- and ssd_b only when the mobile accepts the Commit Request.
ALTER TABLE subscriber_identities
    ADD COLUMN pending_a_key BYTEA NULL,
    ADD COLUMN pending_ssd_a BYTEA NULL,
    ADD COLUMN pending_ssd_b BYTEA NULL;

ALTER TABLE subscriber_identities
    ADD CONSTRAINT chk_identity_pending_a_key_len
        CHECK (pending_a_key IS NULL OR octet_length(pending_a_key) = 8),
    ADD CONSTRAINT chk_identity_pending_ssd_a_len
        CHECK (pending_ssd_a IS NULL OR octet_length(pending_ssd_a) = 8),
    ADD CONSTRAINT chk_identity_pending_ssd_b_len
        CHECK (pending_ssd_b IS NULL OR octet_length(pending_ssd_b) = 8);
