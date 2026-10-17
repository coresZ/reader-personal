-- The chapter cache moved to files in `storage/cache`; these two tables have
-- not been read or written since the initial schema and only retain empty
-- pages in existing databases.
DROP TABLE IF EXISTS book_cache;
DROP TABLE IF EXISTS chapter_cache;
