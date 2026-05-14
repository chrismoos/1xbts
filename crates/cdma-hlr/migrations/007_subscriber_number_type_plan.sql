-- Per-subscriber AWIM Calling Party Number config (C.S0005-E 3.7.5.10).

CREATE TYPE number_type AS ENUM (
    'unknown',
    'international',
    'national',
    'network_specific',
    'subscriber',
    'abbreviated'
);

CREATE TYPE number_plan AS ENUM (
    'unknown',
    'isdn_e164',
    'data',
    'telex',
    'private'
);

ALTER TABLE subscribers
    ADD COLUMN number_type number_type NOT NULL DEFAULT 'network_specific',
    ADD COLUMN number_plan number_plan NOT NULL DEFAULT 'isdn_e164';
