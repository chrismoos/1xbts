ALTER TABLE registration_bindings
    ADD COLUMN IF NOT EXISTS canonical_imsi VARCHAR(15);
