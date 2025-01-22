-- Add migration script here
CREATE TABLE blocks (
    hash TEXT PRIMARY KEY,          -- Corresponds to BlockHash 
    parent_hash TEXT NOT NULL,      -- Parent block hash (BlockHash)
    height BIGINT NOT NULL,         -- Corresponds to BlockHeight (u64)
    timestamp TIMESTAMP NOT NULL,   -- UNIX timestamp (u64)
    UNIQUE (height),                -- Ensure each block height is unique
    CHECK (length(hash) = 64),      -- Ensure the hash is exactly 64
    CHECK (height >= 0)             -- Ensure the height is positive
);

CREATE TYPE transaction_type AS ENUM ('blob_transaction', 'proof_transaction', 'register_contract_transaction', 'stake');
CREATE TYPE transaction_status AS ENUM ('success', 'failure', 'sequenced', 'timed_out');

CREATE TABLE transactions (
    tx_hash TEXT PRIMARY KEY,
    block_hash TEXT NOT NULL REFERENCES blocks(hash) ON DELETE CASCADE,
    index INT NOT NULL,                              -- Index of the transaction within the block
    version INT NOT NULL,
    transaction_type transaction_type NOT NULL,      -- Field to identify the type of transaction (used for joins)
    transaction_status transaction_status NOT NULL   -- Field to identify the status of the transaction
    CHECK (length(tx_hash) = 64)                     -- Ensure the hash is exactly 64
);

CREATE TABLE blobs (
    tx_hash TEXT NOT NULL REFERENCES transactions(tx_hash) ON DELETE CASCADE,  -- Foreign key linking to the BlobTransactions
    blob_index INT NOT NULL,           -- Index of the blob within the transaction
    identity TEXT NOT NULL,            -- Identity field from the original BlobTransaction struct
    contract_name TEXT NOT NULL,       -- Contract name associated with the blob
    data BYTEA NOT NULL,               -- Actual blob data (stored as binary)
    verified BOOLEAN NOT NULL,         -- Field to indicate if the blob is verified
    PRIMARY KEY (tx_hash, blob_index), -- Composite primary key (tx_hash + blob_index) to uniquely identify each blob
    CHECK (blob_index >= 0)            -- Ensure the index is positive
);

-- This table stores actual proofs, which may not be present in all indexers
CREATE TABLE proofs (
    tx_hash TEXT PRIMARY KEY REFERENCES transactions(tx_hash) ON DELETE CASCADE,
    proof BYTEA NOT NULL
);

-- This table stores one line for each hyle output in a VerifiedProof
CREATE TABLE blob_proof_outputs (
    proof_tx_hash TEXT REFERENCES transactions(tx_hash) ON DELETE CASCADE,
    blob_tx_hash TEXT NOT NULL,         -- Foreign key linking to the BlobTransactions
    blob_index INT NOT NULL,            -- Index of the blob within the transaction
    blob_proof_output_index INT NOT NULL, -- Index of the blob proof output within the proof
    contract_name TEXT NOT NULL,       -- Contract name associated with the blob
    hyle_output JSONB NOT NULL,        -- Additional metadata stored in JSONB format
    settled BOOLEAN NOT NULL,       -- Was this blob proof output used in settlement ? 
    PRIMARY KEY (proof_tx_hash, blob_tx_hash, blob_index, blob_proof_output_index),
    FOREIGN KEY (blob_tx_hash) REFERENCES transactions(tx_hash) ON DELETE CASCADE,
    FOREIGN KEY (blob_tx_hash, blob_index) REFERENCES blobs(tx_hash, blob_index) ON DELETE CASCADE,
    UNIQUE (blob_tx_hash, blob_index, blob_proof_output_index)
);

CREATE TABLE contracts (
    tx_hash TEXT PRIMARY KEY REFERENCES transactions(tx_hash) ON DELETE CASCADE,
    verifier TEXT NOT NULL,
    program_id BYTEA NOT NULL,
    state_digest BYTEA NOT NULL,
    contract_name TEXT NOT NULL
);

CREATE TABLE contract_state (
    contract_name TEXT NOT NULL,                                          -- Name of the contract
    block_hash TEXT NOT NULL REFERENCES blocks(hash) ON DELETE CASCADE,   -- Block where the state is captured
    state_digest BYTEA NOT NULL,                                          -- The contract state stored in JSON format for flexibility
    PRIMARY KEY (contract_name, block_hash)
);
