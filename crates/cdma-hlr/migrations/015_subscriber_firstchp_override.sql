-- Per-subscriber FIRSTCHP override used during OTASP `*228` NAM download.
-- FIRSTCHP is the analog first paging/control channel in the CDMA/Analog NAM
-- block. Null means OTASP preserves the handset's existing value.
ALTER TABLE subscribers
    ADD COLUMN firstchp_override INTEGER NULL;

ALTER TABLE subscribers
    ADD CONSTRAINT chk_subscriber_firstchp_override
    CHECK (firstchp_override IS NULL OR firstchp_override BETWEEN 0 AND 2047);
