-- The registration binding records the base station a mobile is served by.
-- Rename the column from serving_node_id to match the base station naming.
ALTER TABLE registration_bindings RENAME COLUMN serving_node_id TO serving_bs_id;
