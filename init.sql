-- init.sql

CREATE TABLE broadcast (
    id BYTEA PRIMARY KEY,
    channel TEXT NOT NULL, -- peers query archives by channel, never by topic
    topic TEXT NOT NULL,
    data BYTEA NOT NULL
);

CREATE INDEX broadcast_channel ON broadcast (channel);

CREATE TABLE id_shares (
   id BIGINT PRIMARY KEY, --user_id
   enc_share BYTEA NOT NULL,
   commits BYTEA NOT NULL
);

CREATE TABLE peer_shares (
    peer_id BYTEA PRIMARY KEY, --peer_id
    enc_share BYTEA NOT NULL,
    commits BYTEA NOT NULL
);

CREATE TABLE group_enc (
    id BYTEA PRIMARY KEY,
    group_id BYTEA NOT NULL,
    data BYTEA NOT NULL,
    nonce BYTEA NOT NULL
);

CREATE TABLE group_table (
    id BYTEA PRIMARY KEY,
    group_number INTEGER NOT NULL,
    data BYTEA NOT NULL
);

CREATE TABLE proposal (
    id BYTEA PRIMARY KEY,
    group_id BYTEA NOT NULL,
    data BYTEA NOT NULL,
    epoch BIGINT NOT NULL
);

CREATE TABLE commit (
    id BYTEA PRIMARY KEY,
    group_id BYTEA NOT NULL,
    commit BYTEA NOT NULL,
    welcome_option BYTEA,
    epoch BIGINT NOT NULL
);

-- Opening requests awaiting this archive operator's decision (also created by the archive at startup)
CREATE TABLE IF NOT EXISTS pending_opens (
    id TEXT PRIMARY KEY,
    signature BYTEA NOT NULL,
    received_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
