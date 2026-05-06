CREATE SCHEMA IF NOT EXISTS hlr;
CREATE SCHEMA IF NOT EXISTS smsc;

COMMENT ON DATABASE "1xbts" IS '1xBTS development database';
COMMENT ON SCHEMA hlr IS '1xBTS HLR subscriber and registration state';
COMMENT ON SCHEMA smsc IS '1xBTS SMSC submission and delivery state';
