-- daily.dev-modeled `post` table (columns mirror dailydotdev/daily-api Post entity:
-- title, summary, tagsStr, sourceId, views/upvotes/comments/score/trending,
-- metadataChangedAt watermark) + a pgvector column for semantic search.
CREATE EXTENSION IF NOT EXISTS vector;
CREATE TABLE IF NOT EXISTS post (
  id text PRIMARY KEY, title text, summary text, tags_str text, source_id text,
  type text default 'article', views int default 0, upvotes int default 0,
  comments int default 0, score int default 0, trending int,
  visible boolean default true, deleted boolean default false,
  created_at timestamptz default now(), metadata_changed_at timestamptz default now(),
  embedding vector(768)
);
-- mirror daily.dev: bump the change-watermark on any update
CREATE OR REPLACE FUNCTION touch_mc() RETURNS trigger AS $BODY$
BEGIN NEW.metadata_changed_at = now(); RETURN NEW; END; $BODY$ LANGUAGE plpgsql;
DROP TRIGGER IF EXISTS post_touch ON post;
CREATE TRIGGER post_touch BEFORE UPDATE ON post FOR EACH ROW EXECUTE FUNCTION touch_mc();
-- logical replication: capture the full row image for UPDATE/DELETE
ALTER TABLE post REPLICA IDENTITY FULL;
-- the CDC slot (built-in test_decoding plugin; swap for wal2json/pgoutput/Debezium in prod)
SELECT pg_create_logical_replication_slot('xerj_slot','test_decoding')
  WHERE NOT EXISTS (SELECT 1 FROM pg_replication_slots WHERE slot_name='xerj_slot');
