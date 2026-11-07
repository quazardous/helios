-- darktable release-5.2.0 (data schema 13), src/common/database.c:
-- _create_data_schema, then upgrade step 12 -> 13 and the metadata keys
-- it inserts (`key` = their index); the tables the importer reads.
CREATE TABLE data.db_info (key VARCHAR PRIMARY KEY, value VARCHAR);
CREATE TABLE data.tags (id INTEGER PRIMARY KEY, name VARCHAR, synonyms VARCHAR, flags INTEGER);
CREATE UNIQUE INDEX data.tags_name_idx ON tags (name);
CREATE TABLE data.meta_data (key INTEGER PRIMARY KEY, tagname VARCHAR, name VARCHAR, internal INTEGER, visible INTEGER, private INTEGER, display_order INTEGER);
CREATE UNIQUE INDEX data.meta_data_tagname_idx ON meta_data (tagname);
CREATE UNIQUE INDEX data.meta_data_name_idx ON meta_data (name);
INSERT INTO data.meta_data VALUES (0, 'Xmp.dc.creator', 'creator', 0, 1, 0, 2);
INSERT INTO data.meta_data VALUES (1, 'Xmp.dc.publisher', 'publisher', 0, 1, 0, 3);
INSERT INTO data.meta_data VALUES (2, 'Xmp.dc.title', 'title', 0, 1, 0, 0);
INSERT INTO data.meta_data VALUES (3, 'Xmp.dc.description', 'description', 0, 1, 0, 1);
INSERT INTO data.meta_data VALUES (4, 'Xmp.dc.rights', 'rights', 0, 1, 0, 4);
INSERT INTO data.meta_data VALUES (5, 'Xmp.acdsee.notes', 'notes', 0, 1, 0, 5);
INSERT INTO data.meta_data VALUES (6, 'Xmp.darktable.version_name', 'version name', 0, 1, 0, 6);
INSERT INTO data.meta_data VALUES (7, 'Xmp.darktable.image_id', 'image id', 1, 0, 0, 7);
INSERT INTO data.meta_data VALUES (8, 'Xmp.xmpMM.PreservedFileName', 'preserved filename', 0, 1, 0, 8);
