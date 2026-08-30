CREATE TABLE product_metadata (
    singleton INTEGER PRIMARY KEY NOT NULL CHECK (singleton = 1),
    application TEXT NOT NULL,
    application_version TEXT NOT NULL,
    schema_revision INTEGER NOT NULL,
    schema_sha256 TEXT NOT NULL
);

CREATE TABLE jobs (
    id TEXT PRIMARY KEY,
    source_asset_id TEXT NOT NULL,
    source_resource_id TEXT NOT NULL,
    media_kind TEXT NOT NULL,
    role TEXT NOT NULL,
    file_path TEXT NOT NULL,
    filename TEXT NOT NULL,
    mime_type TEXT NOT NULL,
    source_created_at_ms INTEGER NOT NULL,
    modified_ms INTEGER NOT NULL,
    source_size INTEGER NOT NULL,
    metadata_json TEXT,
    remove_source_after_prepare INTEGER NOT NULL DEFAULT 0
        CHECK (remove_source_after_prepare IN (0, 1)),
    state TEXT NOT NULL CHECK (state IN (
        'discovered',
        'preparing',
        'ready',
        'uploading',
        'complete',
        'retry_wait',
        'failed'
    )),
    prepared_json TEXT,
    upload_id TEXT,
    retry_count INTEGER NOT NULL DEFAULT 0,
    next_retry_ms INTEGER NOT NULL DEFAULT 0,
    error TEXT,
    updated_at_ms INTEGER NOT NULL,
    UNIQUE(source_asset_id, source_resource_id)
);

CREATE TABLE job_parts (
    job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    part_index INTEGER NOT NULL,
    uploaded INTEGER NOT NULL DEFAULT 0 CHECK (uploaded IN (0, 1)),
    PRIMARY KEY(job_id, part_index)
);
