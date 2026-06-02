-- OTASP `*228` session history. events_proto holds the full timeline
-- as a prost-encoded `events.v1.RecordedEvents` blob; the other columns
-- are cached for list/filter queries.

CREATE TABLE IF NOT EXISTS otasp_sessions (
    session_id       UUID PRIMARY KEY,
    -- Nullable so HlrMiss sessions and subscriber deletions don't lose history.
    subscriber_id    UUID NULL REFERENCES subscribers(subscriber_id) ON DELETE SET NULL,
    -- 32-bit ESN stored as bigint; null when MS used MEID only.
    esn              BIGINT NULL,
    meid             VARCHAR(20) NULL,
    started_at       TIMESTAMPTZ NOT NULL,
    ended_at         TIMESTAMPTZ NULL,
    outcome          SMALLINT NOT NULL,
    feature_code     VARCHAR(16) NULL,
    service_option   INT NULL,
    completed_blocks INT NOT NULL DEFAULT 0,
    event_count      INT NOT NULL DEFAULT 0,
    events_proto     BYTEA NOT NULL,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_otasp_sessions_subscriber_started
  ON otasp_sessions (subscriber_id, started_at DESC)
  WHERE subscriber_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_otasp_sessions_esn
  ON otasp_sessions (esn) WHERE esn IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_otasp_sessions_meid
  ON otasp_sessions (meid) WHERE meid IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_otasp_sessions_started_at
  ON otasp_sessions (started_at DESC);
