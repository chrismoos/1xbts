-- Allow SMS submissions to target by ESN or IMSI instead of phone number.
-- destination_number becomes optional; two new columns carry the alternatives.

SET search_path = smsc;

ALTER TABLE sms_submissions
    ALTER COLUMN destination_number DROP NOT NULL;

ALTER TABLE sms_submissions
    ADD COLUMN destination_esn  BIGINT       NULL,
    ADD COLUMN destination_imsi VARCHAR(15)  NULL;

CREATE INDEX idx_sms_destination_esn
    ON sms_submissions (destination_esn)
    WHERE destination_esn IS NOT NULL;

CREATE INDEX idx_sms_destination_imsi
    ON sms_submissions (destination_imsi)
    WHERE destination_imsi IS NOT NULL;

ALTER TABLE sms_submissions
    ADD CONSTRAINT chk_destination_set CHECK (
        destination_number IS NOT NULL
        OR destination_esn  IS NOT NULL
        OR destination_imsi IS NOT NULL
    );

-- delivery attempts may now target unprovisioned mobiles with no subscriber record
ALTER TABLE sms_delivery_attempts
    ALTER COLUMN target_subscriber_id DROP NOT NULL;
