-- fixtures/test_data.sql

-- Inserting test data for the blocks table
INSERT INTO blocks (hash, parent_hash, height, timestamp)
VALUES
    ('block1aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'block0aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 1, to_timestamp(1632938400)),  -- Block 1
    ('block2aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'block1aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 2, to_timestamp(1632938460));  -- Block 2

-- Inserting test data for the transactions table
INSERT INTO transactions (tx_hash, parent_dp_hash, block_hash, index, version, transaction_type, transaction_status)
VALUES
    ('test_tx_hash_0aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'dp_hashaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', NULL, NULL, 1, 'blob_transaction', 'waiting_dissemination'),  -- Transaction 1 (contract_registration)
    ('test_tx_hash_1aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'dp_hashaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'block2aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 0, 1, 'blob_transaction', 'success'),  -- Transaction 1 (contract_registration)
    ('test_tx_hash_2aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'dp_hashaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'block2aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 1, 1, 'blob_transaction', 'success'),               -- Transaction 2 (blob)
    ('test_tx_hash_3aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'dp_hashaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'block2aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 2, 1, 'proof_transaction', 'success'),              -- Transaction 3 (proof)
    ('test_tx_hash_4aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'dp_hashaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'block2aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 3, 1, 'blob_transaction', 'sequenced');             -- Transaction 4 (blob)

-- Inserting test data for the blob_transactions table
INSERT INTO blobs (tx_hash, parent_dp_hash, blob_index, identity, contract_name, data, verified)
VALUES
    ('test_tx_hash_2aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'dp_hashaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 0, 'identity_1', 'contract_1', '{"data": "blob_data_1"}', false),  -- Blob Transaction 2
    ('test_tx_hash_4aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'dp_hashaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 0, 'identity_1', 'contract_1', '{"data": "blob_data_4"}', false);  -- Blob Transaction 2

-- Inserting test data for the proof_transactions table
INSERT INTO proofs (tx_hash, parent_dp_hash, proof)
VALUES
    ('test_tx_hash_3aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'dp_hashaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', convert_to('proof_data_2', 'UTF-8'));  -- Proof Transaction 3

-- Inserting test data for the blob_proof_outputs table
INSERT INTO blob_proof_outputs (proof_tx_hash, proof_parent_dp_hash, blob_tx_hash, blob_parent_dp_hash, blob_index, blob_proof_output_index, contract_name, hyle_output, settled)
VALUES
    ('test_tx_hash_3aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'dp_hashaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'test_tx_hash_2aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'dp_hashaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 0, 0, 'contract_1', '{}', true);  -- Proof Transaction 3

-- Inserting test data for the contracts table
INSERT INTO contracts (tx_hash, parent_dp_hash, verifier, program_id, state_digest, contract_name)
VALUES
    ('test_tx_hash_1aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'dp_hashaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'verifier_1', convert_to('program_id_1', 'UTF-8'), convert_to('state_digest_1', 'UTF-8'), 'contract_1');  -- Contract 1

-- Inserting test data for the contract_state table
INSERT INTO contract_state (contract_name, block_hash, state_digest)
VALUES
    ('contract_1', 'block1aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', convert_to('state_digest_1', 'UTF-8')),     -- State for Contract 1
    ('contract_1', 'block2aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', convert_to('state_digest_1Bis', 'UTF-8'));  -- State for Contract 2
