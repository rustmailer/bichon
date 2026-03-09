-- =========================
-- envelopes (main table)
-- =========================
CREATE TABLE IF NOT EXISTS envelopes (
    -- internal id (tantivy f_id)
    id                UBIGINT NOT NULL,

    -- account / mailbox / uid
    account_id        UBIGINT NOT NULL,
    mailbox_id        UBIGINT NOT NULL,
    uid               UBIGINT NOT NULL,

    -- headers / content
    subject           TEXT,
    body              TEXT,

    sender            TEXT,
    recipients        VARCHAR[],
    cc                VARCHAR[],
    bcc               VARCHAR[],

    -- dates
    sent_at           BIGINT,
    received_at       BIGINT,

    -- size
    size_bytes        UBIGINT,

    -- thread
    thread_id         UBIGINT,

    -- message-id
    message_id        TEXT,

    -- attachment summary
    has_attachment    BOOLEAN NOT NULL,
    attachment_count  INTEGER NOT NULL CHECK (attachment_count >= 0),
    tags              VARCHAR[],
    shard_id          UBIGINT NOT NULL
);

CREATE INDEX idx_env_mailbox_sent ON envelopes(account_id, mailbox_id, sent_at);

-- =========================
-- envelope_attachments
--
-- Normalized attachment metadata for fast filtering and UI display
-- =========================
CREATE TABLE IF NOT EXISTS envelope_attachments (
    -- Reference to envelopes.id
    envelope_id      UBIGINT NOT NULL,
    account_id        UBIGINT NOT NULL,
    mailbox_id        UBIGINT NOT NULL,
    -- Original attachment filename (for display)
    filename         TEXT NOT NULL,

    -- Normalized file extension (lowercase, without dot)
    extension        TEXT NOT NULL,

    -- Extension category (document / image / archive / ...)
    ext_category     TEXT NOT NULL,
    content_type     TEXT NOT NULL,
    -- Attachment size in bytes
    -- 0 if unknown
    size_bytes       UBIGINT NOT NULL,
    shard_id      UBIGINT NOT NULL
);


CREATE INDEX IF NOT EXISTS idx_attachments_mailbox_id ON envelope_attachments (mailbox_id);
