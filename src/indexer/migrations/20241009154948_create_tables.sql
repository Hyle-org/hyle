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

CREATE TYPE transaction_type AS ENUM ('blob_transaction', 'proof_transaction', 'stake');
CREATE TYPE transaction_status AS ENUM ('data_proposal_created','waiting_dissemination','success', 'failure', 'sequenced', 'timed_out');

CREATE TABLE transactions (
    parent_dp_hash TEXT NOT NULL,                           -- Data Proposal hash
    tx_hash TEXT NOT NULL,
    version INT NOT NULL,
    transaction_type transaction_type NOT NULL,      -- Field to identify the type of transaction (used for joins)
    transaction_status transaction_status NOT NULL,  -- Field to identify the status of the transaction
    block_hash TEXT REFERENCES blocks(hash) ON DELETE CASCADE,
    index INT,                              -- Index of the transaction within the block
    PRIMARY KEY (parent_dp_hash, tx_hash),
    CHECK (length(tx_hash) = 64)
);

CREATE TABLE blobs (
    parent_dp_hash TEXT NOT NULL,  -- Foreign key linking to the parent_dp_hash BlobTransactions
    tx_hash TEXT NOT NULL,  -- Foreign key linking to the tx_hash BlobTransactions
    
    blob_index INT NOT NULL,           -- Index of the blob within the transaction
    identity TEXT NOT NULL,            -- Identity field from the original BlobTransaction struct
    contract_name TEXT NOT NULL,       -- Contract name associated with the blob
    data BYTEA NOT NULL,               -- Actual blob data (stored as binary)
    verified BOOLEAN NOT NULL,         -- Field to indicate if the blob is verified
    PRIMARY KEY (parent_dp_hash, tx_hash, blob_index), -- Composite primary key (parent_dp_hash + tx_hash + blob_index) to uniquely identify each blob
    CHECK (blob_index >= 0),           -- Ensure the index is positive
    FOREIGN KEY (parent_dp_hash, tx_hash) REFERENCES transactions(parent_dp_hash, tx_hash) ON DELETE CASCADE
);

-- This table stores actual proofs, which may not be present in all indexers
CREATE TABLE proofs (
    tx_hash TEXT NOT NULL,    
    parent_dp_hash TEXT NOT NULL,    
    proof BYTEA NOT NULL,
    FOREIGN KEY (parent_dp_hash, tx_hash) REFERENCES transactions(parent_dp_hash, tx_hash) ON DELETE CASCADE,
    PRIMARY KEY (tx_hash, parent_dp_hash)
);

-- This table stores one line for each hyle output in a VerifiedProof
CREATE TABLE blob_proof_outputs (
    blob_parent_dp_hash  TEXT NOT NULL,         -- Foreign key linking to the BlobTransactions    
    blob_tx_hash  TEXT NOT NULL,         -- Foreign key linking to the BlobTransactions    
    proof_parent_dp_hash TEXT NOT NULL,
    proof_tx_hash TEXT NOT NULL,
    blob_index INT NOT NULL,            -- Index of the blob within the transaction
    blob_proof_output_index INT NOT NULL, -- Index of the blob proof output within the proof
    contract_name TEXT NOT NULL,       -- Contract name associated with the blob
    hyle_output JSONB NOT NULL,        -- Additional metadata stored in JSONB format
    settled BOOLEAN NOT NULL,       -- Was this blob proof output used in settlement ? 
    PRIMARY KEY (proof_parent_dp_hash, proof_tx_hash, blob_parent_dp_hash, blob_tx_hash, blob_index, blob_proof_output_index),
    FOREIGN KEY (blob_parent_dp_hash, blob_tx_hash, blob_index) REFERENCES blobs(parent_dp_hash, tx_hash, blob_index) ON DELETE CASCADE,
    FOREIGN KEY (blob_tx_hash, blob_parent_dp_hash) REFERENCES transactions(tx_hash, parent_dp_hash) ON DELETE CASCADE,
    FOREIGN KEY (proof_tx_hash, proof_parent_dp_hash) REFERENCES transactions(tx_hash, parent_dp_hash) ON DELETE CASCADE,
    UNIQUE (blob_parent_dp_hash, blob_tx_hash, blob_index, blob_proof_output_index)
);

CREATE TABLE contracts (
    tx_hash TEXT NOT NULL,
    parent_dp_hash TEXT NOT NULL,
    verifier TEXT NOT NULL,
    program_id BYTEA NOT NULL,
    state_digest BYTEA NOT NULL,
    contract_name TEXT PRIMARY KEY NOT NULL,
    FOREIGN KEY (parent_dp_hash, tx_hash) REFERENCES transactions(parent_dp_hash, tx_hash) ON DELETE CASCADE
);

CREATE TABLE contract_state (
    contract_name TEXT NOT NULL,                                          -- Name of the contract
    block_hash TEXT NOT NULL REFERENCES blocks(hash) ON DELETE CASCADE,   -- Block where the state is captured
    state_digest BYTEA NOT NULL,                                          -- The contract state stored in JSON format for flexibility
    PRIMARY KEY (contract_name, block_hash)
);

CREATE TABLE transaction_state_events (
    block_hash TEXT NOT NULL REFERENCES blocks(hash) ON DELETE CASCADE,
    index INT,
    tx_hash TEXT NOT NULL,
    parent_dp_hash TEXT NOT NULL,
    FOREIGN KEY (tx_hash, parent_dp_hash) REFERENCES transactions(tx_hash, parent_dp_hash) ON DELETE CASCADE,
    
    events JSONB NOT NULL
);
