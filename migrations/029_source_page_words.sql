-- Word boxes of a source page, written at import for the table shapes (src/sources/page_words.rs
-- holds the versioned encoding and validates a row again when it is read). Append only like the
-- other import artefacts: never evidence, never shown as text of its own, not indexed for search.
-- A page with no words is a row holding the version byte alone, so a re-run of import-source
-- fills only the pages that have no row. The size bound is 20,000 words x (256 + 18) bytes + 1.
CREATE TABLE source_page_words (
 page_id INTEGER PRIMARY KEY REFERENCES source_pages(id),
 words BLOB NOT NULL CHECK(length(words) BETWEEN 1 AND 5480001 AND substr(words,1,1)=x'01')
) STRICT;
-- OR REPLACE would delete the old row without firing the delete trigger, so a second row for a
-- page is refused on insert.
CREATE TRIGGER source_page_words_insert BEFORE INSERT ON source_page_words WHEN EXISTS(SELECT 1 FROM source_page_words WHERE page_id=NEW.page_id)
BEGIN SELECT RAISE(ABORT,'Page words are written once'); END;
CREATE TRIGGER source_page_words_update BEFORE UPDATE ON source_page_words BEGIN SELECT RAISE(ABORT,'Page words are append only'); END;
CREATE TRIGGER source_page_words_delete BEFORE DELETE ON source_page_words BEGIN SELECT RAISE(ABORT,'Page words are retained'); END;
PRAGMA user_version=29;
