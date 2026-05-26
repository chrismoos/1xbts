-- Carry C.S0015-B teleservice ID and an opaque User Data payload alongside
-- each submission. Lets the BSC dispatch non-WMT teleservices (e.g. 0x1004
-- CATPT WAP Push for MMS M-Notification.ind) and pass through binary PDUs
-- that the encoder must emit as MSG_ENCODING=0x00 octet.

SET search_path = smsc;

ALTER TABLE sms_submissions
    ADD COLUMN teleservice_id INTEGER NULL,
    ADD COLUMN raw_user_data  BYTEA   NULL;
