CREATE TABLE subscriber_ringtones (
    subscriber_id     UUID         NOT NULL REFERENCES subscribers(subscriber_id) ON DELETE CASCADE,
    codec             VARCHAR(16)  NOT NULL,
    encoded_frames    BYTEA        NOT NULL,
    frame_count       INTEGER      NOT NULL,
    duration_ms       INTEGER      NOT NULL,
    original_filename VARCHAR(255) NOT NULL,
    uploaded_at       TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    PRIMARY KEY (subscriber_id, codec)
);
