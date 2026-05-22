-- init.sql

CREATE TABLE broadcast (
    id BYTEA PRIMARY KEY,
    topic TEXT NOT NULL,
    data BYTEA NOT NULL
);

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
