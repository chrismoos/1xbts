CREATE TABLE IF NOT EXISTS sms_submissions (
    sms_id UUID PRIMARY KEY,
    originating_number VARCHAR(20) NOT NULL,
    destination_number VARCHAR(20) NOT NULL,
    originating_subscriber_id UUID,
    destination_subscriber_id UUID,
    text TEXT NOT NULL,
    state VARCHAR(30) NOT NULL DEFAULT 'accepted',
    failure_reason TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_sms_destination ON sms_submissions(destination_number);
CREATE INDEX IF NOT EXISTS idx_sms_state ON sms_submissions(state);

CREATE TABLE IF NOT EXISTS sms_delivery_attempts (
    sms_delivery_attempt_id UUID PRIMARY KEY,
    sms_id UUID NOT NULL REFERENCES sms_submissions(sms_id) ON DELETE CASCADE,
    attempt_number INTEGER NOT NULL DEFAULT 1,
    state VARCHAR(30) NOT NULL DEFAULT 'queued',
    target_subscriber_id UUID NOT NULL,
    failure_reason TEXT,
    requested_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_delivery_sms_id ON sms_delivery_attempts(sms_id);
