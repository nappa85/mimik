CREATE TABLE sources (
    id INTEGER PRIMARY KEY,
    bundle_hash TEXT NOT NULL UNIQUE,
    source_id TEXT NOT NULL,
    title TEXT NOT NULL,
    system_id TEXT NOT NULL,
    edition TEXT NOT NULL,
    language TEXT NOT NULL,
    pdf_hash TEXT NOT NULL,
    pdf BLOB NOT NULL,
    raw_json BLOB NOT NULL,
    prepared_json BLOB NOT NULL
);
CREATE TABLE source_pages (
    id INTEGER PRIMARY KEY,
    source_id INTEGER NOT NULL REFERENCES sources(id),
    pdf_page INTEGER NOT NULL,
    png BLOB NOT NULL,
    printed_label TEXT NOT NULL DEFAULT '',
    complete INTEGER NOT NULL DEFAULT 0,
    revision INTEGER NOT NULL DEFAULT 1,
    UNIQUE(source_id, pdf_page)
);
CREATE TABLE source_units (
    id INTEGER PRIMARY KEY,
    page_id INTEGER NOT NULL REFERENCES source_pages(id),
    block_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    polygon TEXT NOT NULL,
    raw_html TEXT NOT NULL,
    raw_text TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'unreviewed' CHECK (state IN ('unreviewed', 'accepted', 'corrected', 'rejected')),
    correction TEXT NOT NULL DEFAULT '',
    revision INTEGER NOT NULL DEFAULT 1,
    UNIQUE(page_id, block_id)
);
CREATE TABLE source_review_events (
    id INTEGER PRIMARY KEY,
    page_id INTEGER NOT NULL REFERENCES source_pages(id),
    unit_id INTEGER REFERENCES source_units(id),
    actor TEXT NOT NULL,
    rationale TEXT NOT NULL,
    state TEXT NOT NULL,
    correction TEXT NOT NULL,
    previous_revision INTEGER NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
PRAGMA user_version = 3;
