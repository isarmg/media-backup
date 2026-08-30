ALTER TABLE uploads ADD COLUMN commit_state TEXT NOT NULL DEFAULT 'receiving'
    CHECK (commit_state IN (
        'receiving',
        'commit_started',
        'finalizing',
        'committed',
        'unknown',
        'failed'
    ));

ALTER TABLE uploads ADD COLUMN commit_staged_key TEXT;
ALTER TABLE uploads ADD COLUMN commit_final_key TEXT;
ALTER TABLE uploads ADD COLUMN commit_account_path TEXT;
ALTER TABLE uploads ADD COLUMN commit_expected_size INTEGER;
ALTER TABLE uploads ADD COLUMN commit_expected_blake3 TEXT;
ALTER TABLE uploads ADD COLUMN commit_blob_id TEXT;
ALTER TABLE uploads ADD COLUMN commit_resource_id TEXT;
ALTER TABLE uploads ADD COLUMN commit_deduplicated INTEGER NOT NULL DEFAULT 0
    CHECK (commit_deduplicated IN (0, 1));
ALTER TABLE uploads ADD COLUMN commit_error TEXT;
ALTER TABLE uploads ADD COLUMN commit_started_at TEXT;
ALTER TABLE uploads ADD COLUMN finalized_at TEXT;

UPDATE uploads
SET commit_state = CASE
        WHEN state = 'complete' THEN 'committed'
        ELSE 'receiving'
    END;

CREATE INDEX uploads_commit_reconcile_idx
    ON uploads(commit_state, updated_at);
