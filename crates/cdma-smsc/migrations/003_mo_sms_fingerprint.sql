ALTER TABLE sms_submissions
    ADD COLUMN IF NOT EXISTS mo_teleservice_id INTEGER,
    ADD COLUMN IF NOT EXISTS mo_message_type INTEGER,
    ADD COLUMN IF NOT EXISTS mo_message_id INTEGER;

CREATE INDEX IF NOT EXISTS idx_sms_mo_fingerprint
    ON sms_submissions (
        originating_subscriber_id,
        originating_number,
        destination_number,
        mo_teleservice_id,
        mo_message_type,
        mo_message_id,
        created_at DESC
    );
