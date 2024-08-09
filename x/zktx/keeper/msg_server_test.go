package keeper_test

import (
	"encoding/json"
	"testing"

	"cosmossdk.io/collections"
	"github.com/hyle-org/hyle/x/zktx"
	"github.com/hyle-org/hyle/x/zktx/keeper"
	"github.com/stretchr/testify/require"
)

func TestUnmarshallHyleOutput(t *testing.T) {
	require := require.New(t)
	raw_json := "{\"version\":1,\"initial_state\":[0,0,0,1],\"next_state\":[0,0,0,15],\"identity\":\"\",\"tx_hash\":[1],\"payload_hash\":[0],\"program_outputs\":null}"
	var output zktx.HyleOutput
	err := json.Unmarshal([]byte(raw_json), &output)
	require.NoError(err)
}

func TestMaybeSettleReadinessIgnored(t *testing.T) {
	f := initFixture(t)
	require := require.New(t)

	// Setup
	txHash := []byte("FakeTx")
	payloadHash := []byte("FakePayloadHash")
	contractName := "contract"
	identity := "anon.contract"
	init_state := []byte("FakeInitState")

	f.k.Contracts.Set(f.ctx, contractName, zktx.Contract{
		StateDigest:      init_state,
		NextTxToSettle:   txHash,
		LatestTxReceived: txHash,
	})

	payloadMetadata := zktx.PayloadMetadata{
		PayloadHash:  payloadHash,
		ContractName: contractName,
		Identity:     identity,
		Verified:     false,
	}
	f.k.ProvenPayload.Set(f.ctx, collections.Join(txHash, uint32(0)), payloadMetadata)
	f.k.ProvenPayload.Set(f.ctx, collections.Join(txHash, uint32(1)), payloadMetadata)

	err := keeper.MaybeSettleTx(f.k, f.ctx, txHash)
	require.NoError(err)

	// Check that nothing happened
	has, _ := f.k.SettledTx.Has(f.ctx, txHash)
	require.Equal(has, false)
	contract, err := f.k.Contracts.Get(f.ctx, contractName)
	require.NoError(err)
	require.Equal([]byte("FakeInitState"), contract.StateDigest)

	// Mark both as verified and a proving success.
	// They will still be ignored as they're based on the wrong initial state.
	payloadMetadata.Verified = true
	payloadMetadata.Success = true
	f.k.ProvenPayload.Set(f.ctx, collections.Join(txHash, uint32(0)), payloadMetadata)
	f.k.ProvenPayload.Set(f.ctx, collections.Join(txHash, uint32(1)), payloadMetadata)

	// Check that nothing happened
	has, _ = f.k.SettledTx.Has(f.ctx, txHash)
	require.Equal(has, false)
	contract, err = f.k.Contracts.Get(f.ctx, contractName)
	require.NoError(err)
	require.Equal([]byte("FakeInitState"), contract.StateDigest)

}

func TestMaybeSettleReadinessSuccess(t *testing.T) {
	f := initFixture(t)
	require := require.New(t)

	// Setup
	txHash := []byte("FakeTx1")
	txHash2 := []byte("FakeTx2")
	payloadHash := []byte("FakePayloadHash")
	contractName := "contract"
	identity := "anon.contract"
	init_state := []byte("FakeInitState")

	f.k.Contracts.Set(f.ctx, contractName, zktx.Contract{
		StateDigest:      init_state,
		NextTxToSettle:   txHash,
		LatestTxReceived: txHash2,
	})

	payloadMetadata1 := zktx.PayloadMetadata{
		PayloadHash:  payloadHash,
		ContractName: contractName,
		Identity:     identity,
		Verified:     true,
		Success:      true,
		InitialState: init_state,
		NextState:    []byte("FakeNextState"),
		NextTxHash:   txHash2,
	}
	payloadMetadata2 := zktx.PayloadMetadata{
		PayloadHash:  payloadHash,
		ContractName: contractName,
		Identity:     identity,
		Verified:     true,
		Success:      true,
		InitialState: []byte("FakeNextState"),
		NextState:    []byte("FakeNextNextState"),
		NextTxHash:   txHash2,
	}
	f.k.ProvenPayload.Set(f.ctx, collections.Join(txHash, uint32(0)), payloadMetadata1)
	f.k.ProvenPayload.Set(f.ctx, collections.Join(txHash, uint32(1)), payloadMetadata2)

	payloadMetadata3 := zktx.PayloadMetadata{
		PayloadHash:  payloadHash,
		ContractName: contractName,
		Identity:     identity,
		Verified:     true,
		Success:      true,
		InitialState: []byte("FakeNextNextState"),
		NextState:    []byte("FakeMetaState"),
	}
	f.k.ProvenPayload.Set(f.ctx, collections.Join(txHash2, uint32(0)), payloadMetadata3)

	err := keeper.MaybeSettleTx(f.k, f.ctx, txHash)
	require.NoError(err)

	settled, err := f.k.SettledTx.Get(f.ctx, txHash)
	require.NoError(err)
	require.Equal(settled, true)
	contract, err := f.k.Contracts.Get(f.ctx, contractName)
	require.NoError(err)
	require.Equal([]byte("FakeMetaState"), contract.StateDigest)
	require.Nil(contract.NextTxToSettle)
	require.Nil(contract.LatestTxReceived)
}

func TestMaybeSettleReadinessAllowRetry(t *testing.T) {
	f := initFixture(t)
	require := require.New(t)

	// Setup
	txHash := []byte("FakeTx1")
	txHash2 := []byte("FakeTx2")
	payloadHash := []byte("FakePayloadHash")
	contractName := "contract"
	identity := "anon.contract"
	init_state := []byte("FakeInitState")

	f.k.Contracts.Set(f.ctx, contractName, zktx.Contract{
		StateDigest:      init_state,
		NextTxToSettle:   txHash,
		LatestTxReceived: txHash2,
	})

	payloadMetadata1 := zktx.PayloadMetadata{
		PayloadHash:  payloadHash,
		ContractName: contractName,
		Identity:     identity,
		Verified:     true,
		Success:      true,
		InitialState: init_state,
		NextState:    []byte("FakeNextState"),
		NextTxHash:   txHash2,
	}
	f.k.ProvenPayload.Set(f.ctx, collections.Join(txHash, uint32(0)), payloadMetadata1)

	payloadMetadata2 := zktx.PayloadMetadata{
		PayloadHash:  payloadHash,
		ContractName: contractName,
		Identity:     identity,
		Verified:     true,
		Success:      true,
		InitialState: []byte("InvalidBranch"),
		NextState:    []byte("FakeMetaState"),
	}
	f.k.ProvenPayload.Set(f.ctx, collections.Join(txHash2, uint32(0)), payloadMetadata2)

	err := keeper.MaybeSettleTx(f.k, f.ctx, txHash)
	require.NoError(err)

	settled, err := f.k.SettledTx.Get(f.ctx, txHash)
	require.NoError(err)
	require.Equal(settled, true)

	// The second proof should just be discarded
	settled, err = f.k.SettledTx.Has(f.ctx, txHash2)
	require.NoError(err)
	require.Equal(settled, false)

	contract, err := f.k.Contracts.Get(f.ctx, contractName)
	require.NoError(err)
	require.Equal([]byte("FakeNextState"), contract.StateDigest)
	require.Equal(contract.NextTxToSettle, txHash2)
	require.Equal(contract.LatestTxReceived, txHash2)
}

func TestMaybeSettleAsFailure(t *testing.T) {
	f := initFixture(t)
	require := require.New(t)

	// Setup
	txHash := []byte("FakeTx1")
	payloadHash := []byte("FakePayloadHash")
	contractName := "contract"
	identity := "anon.contract"
	init_state := []byte("FakeInitState")

	f.k.Contracts.Set(f.ctx, contractName, zktx.Contract{
		StateDigest:      init_state,
		NextTxToSettle:   txHash,
		LatestTxReceived: txHash,
	})

	payloadMetadata1 := zktx.PayloadMetadata{
		PayloadHash:  payloadHash,
		ContractName: contractName,
		Identity:     identity,
		Verified:     true,
		Success:      false,
		InitialState: init_state,
		NextState:    []byte("FakeNextState"),
	}
	f.k.ProvenPayload.Set(f.ctx, collections.Join(txHash, uint32(0)), payloadMetadata1)

	err := keeper.MaybeSettleTx(f.k, f.ctx, txHash)
	require.NoError(err)

	settled, err := f.k.SettledTx.Get(f.ctx, txHash)
	require.NoError(err)
	require.Equal(settled, false)
}
