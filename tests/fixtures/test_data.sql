-- fixtures/test_data.sql

-- Inserting test data for the blocks table
INSERT INTO blocks (hash, parent_hash, height, timestamp)
VALUES
    (convert_to('block1aaaaaaaaaaaaaaaaaaaaaaaaaa', 'UTF-8'), convert_to('block0', 'UTF-8'), 1, 1632938400),  -- Block 1
    (convert_to('block2aaaaaaaaaaaaaaaaaaaaaaaaaa', 'UTF-8'), convert_to('block1', 'UTF-8'), 2, 1632938460);  -- Block 2

-- Inserting test data for the transactions table
INSERT INTO transactions (tx_hash, block_hash, tx_index, version, transaction_type, transaction_status)
VALUES
    (convert_to('test_tx_hash_1aaaaaaaaaaaaaaaaaa', 'UTF-8'), convert_to('block2aaaaaaaaaaaaaaaaaaaaaaaaaa', 'UTF-8'), 0, 1, 'RegisterContractTransaction', 'Success'),  -- Transaction 1 (contract_registration)
    (convert_to('test_tx_hash_2aaaaaaaaaaaaaaaaaa', 'UTF-8'), convert_to('block2aaaaaaaaaaaaaaaaaaaaaaaaaa', 'UTF-8'), 1, 1, 'BlobTransaction', 'Success'),              -- Transaction 2 (blob)
    (convert_to('test_tx_hash_3aaaaaaaaaaaaaaaaaa', 'UTF-8'), convert_to('block2aaaaaaaaaaaaaaaaaaaaaaaaaa', 'UTF-8'), 2, 1, 'ProofTransaction', 'Success'),             -- Transaction 3 (proof)
    (convert_to('test_tx_hash_4aaaaaaaaaaaaaaaaaa', 'UTF-8'), convert_to('block2aaaaaaaaaaaaaaaaaaaaaaaaaa', 'UTF-8'), 3, 1, 'BlobTransaction', 'Sequenced');            -- Transaction 4 (blob)

-- Inserting test data for the blob_transactions table
INSERT INTO blobs (tx_hash, blob_index, identity, contract_name, data)
VALUES
    (convert_to('test_tx_hash_2aaaaaaaaaaaaaaaaaa', 'UTF-8'), 0, 'identity_1', 'contract_1', '{"data": "blob_data_1"}'),  -- Blob Transaction 2
    (convert_to('test_tx_hash_4aaaaaaaaaaaaaaaaaa', 'UTF-8'), 0, 'identity_1', 'contract_1', '{"data": "blob_data_4"}');  -- Blob Transaction 2

-- Inserting test data for the proof_transactions table
INSERT INTO proofs (tx_hash, proof)
VALUES
    (convert_to('test_tx_hash_3aaaaaaaaaaaaaaaaaa', 'UTF-8'), convert_to('proof_data_2', 'UTF-8'));  -- Proof Transaction 3

-- Inserting test data for the blob_references table
INSERT INTO blob_references (tx_hash, contract_name, blob_tx_hash, blob_index)
VALUES
    (convert_to('test_tx_hash_3aaaaaaaaaaaaaaaaaa', 'UTF-8'), 'contract_1', convert_to('test_tx_hash_2aaaaaaaaaaaaaaaaaa', 'UTF-8'), 0);  -- Blob Reference for Proof transaction 2

-- Inserting test data for the contracts table
INSERT INTO contracts (tx_hash, owner, verifier, program_id, state_digest, contract_name)
VALUES
    (convert_to('test_tx_hash_1aaaaaaaaaaaaaaaaaa', 'UTF-8'), 'owner_1', 'verifier_1', convert_to('program_id_1', 'UTF-8'), convert_to('state_digest_1', 'UTF-8'), 'contract_1');  -- Contract 1

-- Inserting test data for the contract_state table
INSERT INTO contract_state (contract_name, block_hash, state)
VALUES
    ('contract_1', convert_to('block1aaaaaaaaaaaaaaaaaaaaaaaaaa', 'UTF-8'), convert_to('state_digest_1', 'UTF-8')),  -- State for Contract 1
    ('contract_1', convert_to('block2aaaaaaaaaaaaaaaaaaaaaaaaaa', 'UTF-8'), convert_to('state_digest_1Bis', 'UTF-8'));  -- State for Contract 2
