CREATE TABLE mobiles_seen (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    esn BIGINT,
    imsi_m_s1 BIGINT,
    imsi_m_s2 BIGINT,
    imsi_mcc INTEGER,
    imsi_11_12 INTEGER,
    imsi_class INTEGER,
    mob_p_rev INTEGER,
    first_seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX idx_mobiles_seen_esn ON mobiles_seen (esn) WHERE esn IS NOT NULL;
CREATE UNIQUE INDEX idx_mobiles_seen_imsi ON mobiles_seen (imsi_m_s1, imsi_m_s2) WHERE imsi_m_s1 IS NOT NULL AND imsi_m_s2 IS NOT NULL;
