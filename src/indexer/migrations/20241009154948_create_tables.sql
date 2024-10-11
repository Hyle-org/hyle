-- Add migration script here
CREATE TABLE blocks (
    hash BYTEA PRIMARY KEY,         -- Corresponds to BlockHash (inner Vec<u8>)
    parent_hash BYTEA NOT NULL,     -- Parent block hash (BlockHash)
    height BIGINT NOT NULL,         -- Corresponds to BlockHeight (u64)
    timestamp TIMESTAMP NOT NULL,   -- UNIX timestamp (u64)
    UNIQUE (height),                -- Ensure each block height is unique
    CHECK (octet_length(hash) = 32) -- Ensure the hash is exactly 32 bytes
);

CREATE TYPE transaction_type AS ENUM ('blob_transaction', 'proof_transaction', 'register_contract_transaction');
CREATE TYPE transaction_status AS ENUM ('success', 'failure', 'sequenced');

CREATE TABLE transactions (
    tx_hash BYTEA PRIMARY KEY,
    block_hash BYTEA NOT NULL REFERENCES blocks(hash) ON DELETE CASCADE,
    tx_index INT NOT NULL,
    version INT NOT NULL,
    transaction_type transaction_type NOT NULL,      -- Field to identify the type of transaction (used for joins)
    transaction_status transaction_status NOT NULL   -- Field to identify the status of the transaction
    CHECK (octet_length(tx_hash) = 32) -- Ensure the hash is exactly 32 bytes
);

CREATE TABLE blobs (
    tx_hash BYTEA NOT NULL REFERENCES transactions(tx_hash) ON DELETE CASCADE,  -- Foreign key linking to the BlobTransactions
    blob_index INT NOT NULL,           -- Index of the blob within the transaction
    identity TEXT NOT NULL,            -- Identity field from the original BlobTransaction struct
    contract_name TEXT NOT NULL,       -- Contract name associated with the blob
    data BYTEA NOT NULL,               -- Actual blob data (stored as binary)
    PRIMARY KEY (tx_hash, blob_index)  -- Composite primary key (tx_hash + blob_index) to uniquely identify each blob
);

CREATE TABLE proofs (
    tx_hash BYTEA PRIMARY KEY REFERENCES transactions(tx_hash) ON DELETE CASCADE,
    proof BYTEA NOT NULL
);

CREATE TABLE blob_references (
    id SERIAL PRIMARY KEY,                                       -- Unique ID for each blob reference
    tx_hash BYTEA REFERENCES proofs(tx_hash) ON DELETE CASCADE,  -- Foreign key linking to proof_transactions
    contract_name TEXT NOT NULL,                                 -- Contract name (you could also use BYTEA depending on how you store ContractName)
    blob_tx_hash BYTEA NOT NULL,                                 -- Blob transaction hash
    blob_index INTEGER NOT NULL                                  -- Index of the blob
    -- hyle_output JSONB  -- Optional field for extra data
);

CREATE TABLE contracts (
    tx_hash BYTEA PRIMARY KEY REFERENCES transactions(tx_hash) ON DELETE CASCADE,
    owner TEXT NOT NULL,
    verifier TEXT NOT NULL,
    program_id BYTEA NOT NULL,
    state_digest BYTEA NOT NULL,
    contract_name TEXT NOT NULL
);

CREATE TABLE contract_state (
    contract_name TEXT NOT NULL,                                          -- Name of the contract
    block_hash BYTEA NOT NULL REFERENCES blocks(hash) ON DELETE CASCADE,  -- Block where the state is captured
    state_digest BYTEA NOT NULL,                                          -- The contract state stored in JSON format for flexibility
    PRIMARY KEY (contract_name, block_hash)
);
