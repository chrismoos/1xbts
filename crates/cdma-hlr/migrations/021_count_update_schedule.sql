ALTER TABLE subscriber_identities
    ADD COLUMN count_updated_at TIMESTAMPTZ,
    ADD COLUMN count_pending_id UUID,
    ADD COLUMN count_pending_base SMALLINT,
    ADD COLUMN count_pending_until TIMESTAMPTZ,
    ADD COLUMN count_retry_after TIMESTAMPTZ,
    ADD COLUMN count_completed_id UUID,
    ADD CONSTRAINT count_pending_complete CHECK (
        (count_pending_id IS NULL AND count_pending_base IS NULL AND count_pending_until IS NULL)
        OR (count_pending_id IS NOT NULL AND count_pending_base IS NOT NULL AND count_pending_base BETWEEN 0 AND 63 AND count_pending_until IS NOT NULL)
    );
