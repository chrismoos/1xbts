-- Per-subscriber Service Programming Code used during OTASP `*228`
-- Verify SPC. Null means use the IS-95 default "000000".
ALTER TABLE subscribers
    ADD COLUMN service_programming_code CHAR(6) NULL;

ALTER TABLE subscribers
    ADD CONSTRAINT chk_subscriber_spc_digits
    CHECK (service_programming_code IS NULL OR service_programming_code ~ '^[0-9]{6}$');
