CREATE TABLE IF NOT EXISTS subscribers (
    subscriber_id UUID PRIMARY KEY,
    phone_number VARCHAR(20) NOT NULL UNIQUE,
    display_name VARCHAR(255) NOT NULL DEFAULT '',
    status VARCHAR(20) NOT NULL DEFAULT 'active',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS subscriber_identities (
    subscriber_identity_id UUID PRIMARY KEY,
    subscriber_id UUID NOT NULL REFERENCES subscribers(subscriber_id) ON DELETE CASCADE,
    esn BIGINT,
    imsi_m_s1 BIGINT,
    imsi_m_s2 BIGINT,
    imsi_mcc BIGINT,
    imsi_11_12 BIGINT,
    imsi_class BIGINT,
    is_primary BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_identity_esn ON subscriber_identities(esn) WHERE esn IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_identity_imsi ON subscriber_identities(imsi_m_s1, imsi_m_s2) WHERE imsi_m_s1 IS NOT NULL AND imsi_m_s2 IS NOT NULL;

CREATE TABLE IF NOT EXISTS registration_bindings (
    subscriber_id UUID PRIMARY KEY REFERENCES subscribers(subscriber_id) ON DELETE CASCADE,
    serving_node_id VARCHAR(255) NOT NULL,
    state VARCHAR(30) NOT NULL DEFAULT 'registered',
    fwd_esn BIGINT,
    fwd_imsi_m_s1 BIGINT,
    fwd_imsi_m_s2 BIGINT,
    page_esn BIGINT,
    page_imsi_m_s1 BIGINT,
    page_imsi_m_s2 BIGINT,
    page_mcc BIGINT,
    page_imsi_11_12 BIGINT,
    mob_p_rev BIGINT,
    pgslot BIGINT,
    slot_cycle_index BIGINT,
    last_msg_seq BIGINT,
    last_registered_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
