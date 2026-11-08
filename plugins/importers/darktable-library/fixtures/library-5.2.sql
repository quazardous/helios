-- darktable release-5.2.0 (library schema 57), src/common/database.c:
-- _create_library_schema (the full schema of library 55), then upgrade
-- step 56 -> 57; the tables the importer reads, as created.
CREATE TABLE main.db_info (key VARCHAR PRIMARY KEY, value VARCHAR);
CREATE TABLE main.film_rolls (id INTEGER PRIMARY KEY, access_timestamp INTEGER, folder VARCHAR(1024) NOT NULL);
CREATE INDEX main.film_rolls_folder_index ON film_rolls (folder);
CREATE TABLE main.images (id INTEGER PRIMARY KEY AUTOINCREMENT, group_id INTEGER, film_id INTEGER,  width INTEGER, height INTEGER, filename VARCHAR,  maker_id INTEGER, model_id INTEGER, lens_id INTEGER, camera_id INTEGER,  exposure REAL, aperture REAL, iso REAL, focal_length REAL,  focus_distance REAL, datetime_taken INTEGER, flags INTEGER,  output_width INTEGER, output_height INTEGER, crop REAL,  raw_parameters INTEGER, raw_black INTEGER, raw_maximum INTEGER,  orientation INTEGER, longitude REAL,  latitude REAL, altitude REAL, color_matrix BLOB, colorspace INTEGER,  version INTEGER, max_version INTEGER, write_timestamp INTEGER,  history_end INTEGER, position INTEGER, aspect_ratio REAL, exposure_bias REAL,  import_timestamp INTEGER DEFAULT -1, change_timestamp INTEGER DEFAULT -1,   export_timestamp INTEGER DEFAULT -1, print_timestamp INTEGER DEFAULT -1,   thumb_timestamp INTEGER DEFAULT -1, thumb_maxmip INTEGER DEFAULT 0,   whitebalance_id INTEGER, flash_id INTEGER,   exposure_program_id INTEGER, metering_mode_id INTEGER, FOREIGN KEY(maker_id) REFERENCES makers(id) ON DELETE CASCADE ON UPDATE CASCADE, FOREIGN KEY(model_id) REFERENCES models(id) ON DELETE CASCADE ON UPDATE CASCADE, FOREIGN KEY(lens_id) REFERENCES lens(id) ON DELETE CASCADE ON UPDATE CASCADE, FOREIGN KEY(camera_id) REFERENCES cameras(id) ON DELETE CASCADE ON UPDATE CASCADE, FOREIGN KEY(film_id) REFERENCES film_rolls(id) ON DELETE CASCADE ON UPDATE CASCADE, FOREIGN KEY(group_id) REFERENCES images(id) ON DELETE RESTRICT ON UPDATE CASCADE, FOREIGN KEY(whitebalance_id) REFERENCES whitebalance(id) ON DELETE CASCADE ON UPDATE CASCADE, FOREIGN KEY(flash_id) REFERENCES flash(id) ON DELETE CASCADE ON UPDATE CASCADE, FOREIGN KEY(exposure_program_id) REFERENCES exposure_program(id) ON DELETE CASCADE ON UPDATE CASCADE, FOREIGN KEY(metering_mode_id) REFERENCES metering_mode(id) ON DELETE CASCADE ON UPDATE CASCADE);
CREATE INDEX main.images_film_id_index ON images (film_id, filename);
CREATE TABLE main.tagged_images (imgid INTEGER, tagid INTEGER, position INTEGER, PRIMARY KEY (imgid, tagid),FOREIGN KEY(imgid) REFERENCES images(id) ON UPDATE CASCADE ON DELETE CASCADE);
CREATE TABLE main.color_labels (imgid INTEGER, color INTEGER);
CREATE UNIQUE INDEX main.color_labels_idx ON color_labels (imgid, color);
CREATE TABLE main.meta_data (id INTEGER, key INTEGER, value VARCHAR, FOREIGN KEY(id) REFERENCES images(id) ON DELETE CASCADE ON UPDATE CASCADE);
CREATE UNIQUE INDEX main.metadata_index ON meta_data (id, key, value);
ALTER TABLE main.images ADD COLUMN flash_tagvalue INTEGER DEFAULT -1;
-- the edit's tables, the same from library schema 36 (darktable 4.0) on
CREATE TABLE main.history (imgid INTEGER, num INTEGER, module INTEGER, operation VARCHAR(256), op_params BLOB, enabled INTEGER, blendop_params BLOB, blendop_version INTEGER, multi_priority INTEGER, multi_name VARCHAR(256), multi_name_hand_edited INTEGER, FOREIGN KEY(imgid) REFERENCES images(id) ON UPDATE CASCADE ON DELETE CASCADE);
CREATE INDEX main.history_imgid_op_index ON history (imgid, operation);
CREATE TABLE main.masks_history (imgid INTEGER, num INTEGER, formid INTEGER, form INTEGER, name VARCHAR(256), version INTEGER, points BLOB, points_count INTEGER, source BLOB, FOREIGN KEY(imgid) REFERENCES images(id) ON UPDATE CASCADE ON DELETE CASCADE);
CREATE INDEX main.masks_history_imgid_index ON masks_history (imgid, num);
CREATE TABLE main.module_order (imgid INTEGER PRIMARY KEY, version INTEGER, iop_list VARCHAR);
CREATE TABLE main.history_hash (imgid INTEGER PRIMARY KEY, basic_hash BLOB, auto_hash BLOB, current_hash BLOB, mipmap_hash BLOB, FOREIGN KEY(imgid) REFERENCES images(id) ON UPDATE CASCADE ON DELETE CASCADE);
