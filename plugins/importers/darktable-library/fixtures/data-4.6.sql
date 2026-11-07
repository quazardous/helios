-- darktable release-4.6.0 (data schema 10), src/common/database.c,
-- _create_data_schema: the tables the importer reads, as created.
CREATE TABLE data.db_info (key VARCHAR PRIMARY KEY, value VARCHAR);
CREATE TABLE data.tags (id INTEGER PRIMARY KEY, name VARCHAR, synonyms VARCHAR, flags INTEGER);
CREATE UNIQUE INDEX data.tags_name_idx ON tags (name);
