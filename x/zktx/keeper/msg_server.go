package keeper

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"strings"

	"cosmossdk.io/collections"
	sdk "github.com/cosmos/cosmos-sdk/types"

	"github.com/hyle-org/hyle/x/zktx"
	"github.com/hyle-org/hyle/x/zktx/keeper/gnark"

	"github.com/consensys/gnark/backend/groth16"
)

type msgServer struct {
	k Keeper
}

var _ zktx.MsgServer = msgServer{}

var risczeroVerifierPath = os.Getenv("RISCZERO_VERIFIER_PATH")
var sp1VerifierPath = os.Getenv("SP1_VERIFIER_PATH")
var noirVerifierPath = os.Getenv("NOIR_VERIFIER_PATH")
var cairoVerifierPath = os.Getenv("CAIRO_VERIFIER_PATH")

var txTimeout = int64(1000)

// NewMsgServerImpl returns an implementation of the module MsgServer interface.
func NewMsgServerImpl(keeper Keeper) zktx.MsgServer {
	// By default, assume the hylé repo shares a parent directory with the verifiers repo.
	// They'll still need to be compiled in release mode.
	// Noir expects bun to be installed.
	if risczeroVerifierPath == "" {
		risczeroVerifierPath = "./target/release/risc0-verifier"
	}
	if sp1VerifierPath == "" {
		sp1VerifierPath = "./target/release/sp1-verifier"
	}
	if noirVerifierPath == "" {
		noirVerifierPath = "./verifiers/noir-verifier"
	}
	if cairoVerifierPath == "" {
		cairoVerifierPath = "./target/release/cairo-verifier"
	}
	return &msgServer{k: keeper}
}

func hashPayloads(payloads []*zktx.Payload) ([]byte, error) {
	var concatenatedData []byte

	// Iterate over all payloads and append their Data fields to the slice
	for _, payload := range payloads {
		if payload != nil {
			concatenatedData = append(concatenatedData, payload.Data...)
		}
	}
	payloadsHash := sha256.Sum256(concatenatedData)

	return payloadsHash[:], nil
}

func hashPayloadsFromProofs(payloads []byte) ([]byte, error) {
	payloadsHash := sha256.Sum256(payloads)

	return payloadsHash[:], nil
}

func (ms msgServer) PublishPayloads(goCtx context.Context, msg *zktx.MsgPublishPayloads) (*zktx.MsgPublishPayloadsResponse, error) {
	ctx := sdk.UnwrapSDKContext(goCtx)

	// Compute txHash
	h := sha256.New()
	h.Write(ctx.TxBytes())
	txHash := h.Sum(nil)

	// Setup verification timeout.
	txs, err := ms.k.Timeout.Get(ctx, ctx.BlockHeight()+txTimeout)
	var newTxs zktx.TxTimeout
	if errors.Is(err, collections.ErrNotFound) {
		newTxs = zktx.TxTimeout{
			Txs: [][]byte{txHash},
		}
	} else if err != nil {
		return nil, err
	} else {
		newTxs = txs
		newTxs.Txs = append(newTxs.Txs, txHash)
	}
	ms.k.Timeout.Set(ctx, ctx.BlockHeight()+txTimeout, newTxs)

	validIdentity := msg.Identity == ""

	payloadsHash, err := hashPayloads(msg.Payloads)
	if err != nil {
		return nil, err
	}

	for i, payload := range msg.Payloads {
		contract, err := ms.k.Contracts.Get(ctx, payload.ContractName)
		if err != nil {
			return nil, fmt.Errorf("invalid contract - no state is registered")
		}

		// Check if this contract name matches the identity
		if !validIdentity {
			paths := strings.Split(msg.Identity, ".")
			if len(paths) >= 2 && paths[len(paths)-1] == payload.ContractName {
				validIdentity = true
			}
		}

		// Store enough metadata to check proofs later.
		err = ms.k.ProvenPayload.Set(ctx, collections.Join(txHash, uint32(i)), zktx.PayloadMetadata{
			PayloadsHash: payloadsHash,
			ContractName: payload.ContractName,
			Identity:     msg.Identity,
		})
		if err != nil {
			return nil, err
		}

		// Maintain the linked DAG of pending transactions.
		// If there's no pending TX, just set it.
		if contract.NextTxToSettle == nil {
			contract.NextTxToSettle = txHash
			contract.LatestTxReceived = txHash
		} else {
			// Otherwise we need to update the pending TX to maintain the list
			// NB: there can be several payloads for the same contract in the same TX
			for i := 0; ; i++ {
				payloadMetadata, err := ms.k.ProvenPayload.Get(ctx, collections.Join(contract.LatestTxReceived, uint32(i)))
				if errors.Is(err, collections.ErrNotFound) {
					break
				} else if err != nil {
					return nil, err
				}
				if payloadMetadata.ContractName == payload.ContractName {
					payloadMetadata.NextTxHash = txHash
					err = ms.k.ProvenPayload.Set(ctx, collections.Join(contract.LatestTxReceived, uint32(i)), payloadMetadata)
					if err != nil {
						return nil, err
					}
				}
			}
			contract.LatestTxReceived = txHash
		}
		if err := ms.k.Contracts.Set(ctx, payload.ContractName, contract); err != nil {
			return nil, err
		}

		// TODO: figure out if we want to reemit TX Hash, payload Hash
		// and maybe just give it a UUID ?
		if err := ctx.EventManager().EmitTypedEvent(&zktx.EventPayload{
			ContractName: payload.ContractName,
			PayloadIndex: uint32(i),
			Data:         payload.Data,
		}); err != nil {
			return nil, err
		}
	}
	if !validIdentity {
		return nil, fmt.Errorf("identity does not match any contract name")
	}
	// TODO fees
	return &zktx.MsgPublishPayloadsResponse{}, nil
}

func (ms msgServer) PublishPayloadProof(goCtx context.Context, msg *zktx.MsgPublishPayloadProof) (*zktx.MsgPublishPayloadProofResponse, error) {
	ctx := sdk.UnwrapSDKContext(goCtx)

	// Check if the TX is already settled
	if settled, err := ms.k.SettledTx.Has(ctx, msg.TxHash); err == nil && settled {
		return nil, fmt.Errorf("TX already settled")
	}

	payloadMetadata, err := ms.k.ProvenPayload.Get(ctx, collections.Join(msg.TxHash, msg.PayloadIndex))
	if err != nil {
		return nil, fmt.Errorf("no payload found for this txHash")
	}

	contract, err := ms.k.Contracts.Get(ctx, msg.ContractName)
	if err != nil {
		return nil, fmt.Errorf("invalid contract - no state is registered")
	}

	var objmap zktx.HyleOutput

	if err := extractProof(&objmap, &contract, msg); err != nil {
		return nil, fmt.Errorf("verification failed: %w", err)
	}

	payloadsHash, err := hashPayloadsFromProofs(objmap.Payloads)
	if err != nil {
		return nil, err
	}

	if !bytes.Equal(payloadsHash, payloadMetadata.PayloadsHash) {
		return nil, fmt.Errorf("proof is not related with correct payloads hash")
	}

	if payloadMetadata.Identity != "" && objmap.Identity != payloadMetadata.Identity {
		return nil, fmt.Errorf("proof is not for the correct identity")
	}

	payloadMetadata.InitialState = objmap.InitialState
	payloadMetadata.NextState = objmap.NextState
	payloadMetadata.Success = objmap.Success
	payloadMetadata.Verified = true

	ms.k.ProvenPayload.Set(ctx, collections.Join(msg.TxHash, msg.PayloadIndex), payloadMetadata)

	// TODO: figure out if we want to reemit TX Hash, payload Hash
	// and maybe just give it a UUID ?
	if err := ctx.EventManager().EmitTypedEvent(&zktx.EventPayloadSettled{
		ContractName: msg.ContractName,
		PayloadIndex: msg.PayloadIndex,
		TxHash:       msg.TxHash,
	}); err != nil {
		return nil, fmt.Errorf("event emission failed: %w", err)
	}
	err = MaybeSettleTx(ms.k, ctx, msg.TxHash)
	if err != nil {
		return nil, err
	}

	return &zktx.MsgPublishPayloadProofResponse{}, nil
}

// MaybeSettleTx attemps to settle whole transactions. Public for testing.
func MaybeSettleTx(k Keeper, ctx sdk.Context, txHash []byte) error {
	// Check if this is the next TX to settle.
	for i := 0; ; i++ {
		payloadMetadata, err := k.ProvenPayload.Get(ctx, collections.Join(txHash, uint32(i)))
		if errors.Is(err, collections.ErrNotFound) {
			break
		}
		contract, err := k.Contracts.Get(ctx, payloadMetadata.ContractName)
		if err != nil {
			return fmt.Errorf("invalid contract - no state is registered")
		}
		if !bytes.Equal(contract.NextTxToSettle, txHash) {
			// Not the next TX to settle, do nothing.
			return nil
		}
	}

	// Check if all payloads have been verified
	success := true
	fullyVerified := true
	nextTxHash := make(map[string][]byte)
	// Because we can have several payloads for the same contract in the same TX
	// we need to update the expected state after each payload.
	expectedState := make(map[string][]byte)
	for i := 0; ; i++ {
		payloadMetadata, err := k.ProvenPayload.Get(ctx, collections.Join(txHash, uint32(i)))
		if errors.Is(err, collections.ErrNotFound) {
			break
		}
		nextTxHash[payloadMetadata.ContractName] = payloadMetadata.NextTxHash

		contract, err := k.Contracts.Get(ctx, payloadMetadata.ContractName)
		if err != nil {
			return fmt.Errorf("invalid contract - no state is registered")
		}
		if expectedState[payloadMetadata.ContractName] == nil {
			expectedState[payloadMetadata.ContractName] = contract.StateDigest
		}
		if !bytes.Equal(payloadMetadata.InitialState, expectedState[payloadMetadata.ContractName]) {
			// This proof is based on an incorrect state, so mark it as not verified (this essentially means the proof is rejected)
			payloadMetadata.Verified = false
		}
		// If the payload is not proven, we can't do much.
		if !payloadMetadata.Verified {
			fullyVerified = false
		} else if !payloadMetadata.Success {
			// TODO: figure out side effects?
			// If any payload fail, the whole TX can be rejected.
			success = false
			break
		}
		expectedState[payloadMetadata.ContractName] = payloadMetadata.NextState
	}

	// Can't settle until either a failure or all TXs are proven correct.
	if success && !fullyVerified {
		return nil
	}

	// Settle
	for i := 0; ; i++ {
		payloadMetadata, err := k.ProvenPayload.Get(ctx, collections.Join(txHash, uint32(i)))
		if errors.Is(err, collections.ErrNotFound) {
			break
		} else if err != nil {
			return err
		}
		// Update contract state
		contract, err := k.Contracts.Get(ctx, payloadMetadata.ContractName)
		// This _should_ never happen, but let's handle it gracefully ish regardless.
		if err != nil {
			return fmt.Errorf("invalid contract - no state is registered")
		}

		if success {
			contract.StateDigest = payloadMetadata.NextState
		}
		// This must be idempotent if there are several payloads for the same
		// contract in the same TX.
		contract.NextTxToSettle = payloadMetadata.NextTxHash // may be nil - well formed
		if bytes.Equal(contract.LatestTxReceived, txHash) {
			contract.LatestTxReceived = nil
		}
		if err := k.Contracts.Set(ctx, payloadMetadata.ContractName, contract); err != nil {
			return err
		}

		k.ProvenPayload.Remove(ctx, collections.Join(txHash, uint32(i)))
	}

	// TODO: figure out if we want to reemit block height?
	if err := ctx.EventManager().EmitTypedEvent(&zktx.EventTxSettled{
		TxHash:  txHash,
		Success: success,
	}); err != nil {
		return err
	}

	// Timeout will be automatically ignored when a TX is settled
	k.SettledTx.Set(ctx, txHash, success)

	for _, nextTx := range nextTxHash {
		if nextTx != nil {
			if err := MaybeSettleTx(k, ctx, nextTx); err != nil {
				return err
			}
		}
	}
	return nil
}

func extractProof(objmap *zktx.HyleOutput, contract *zktx.Contract, msg *zktx.MsgPublishPayloadProof) error {
	if contract.Verifier == "risczero" {
		// Save proof to a local file
		err := os.WriteFile("/tmp/risc0-proof.json", msg.Proof, 0644)

		if err != nil {
			return fmt.Errorf("failed to write proof to file: %s", err)
		}

		b16ProgramID := hex.EncodeToString(contract.ProgramId)
		outBytes, err := exec.Command(risczeroVerifierPath, b16ProgramID, "/tmp/risc0-proof.json").Output()
		if err != nil {
			return fmt.Errorf("risczero verifier failed on %s. Exit code: %s", msg.ContractName, err)
		}
		// Then parse data from the verified proof.

		err = json.Unmarshal(outBytes, &objmap)
		if err != nil {
			return fmt.Errorf("failed to unmarshal verifier output: %s", err)
		}

		//proofData = string(outBytes)
	} else if contract.Verifier == "sp1" {
		// Save proof to a local file
		err := os.WriteFile("/tmp/sp1-proof.json", msg.Proof, 0644)

		if err != nil {
			return fmt.Errorf("failed to write proof to file: %s", err)
		}
		b64ProgramID := base64.StdEncoding.EncodeToString(contract.ProgramId)
		outBytes, err := exec.Command(sp1VerifierPath, b64ProgramID, "/tmp/sp1-proof.json").Output()
		if err != nil {
			return fmt.Errorf("sp1 verifier failed on %s. Exit code: %s", msg.ContractName, err)
		}
		// Then parse data from the verified proof.
		err = json.Unmarshal(outBytes, &objmap)
		if err != nil {
			return fmt.Errorf("failed to unmarshal verifier output: %s", err)
		}

		//proofData = string(outBytes)
	} else if contract.Verifier == "noir" {
		// Save proof to a local file
		err := os.WriteFile("/tmp/noir-proof", msg.Proof, 0644)
		if err != nil {
			return fmt.Errorf("failed to write proof to file: %s", err)
		}
		// Save vKey to a local file
		f, err := os.Create("/tmp/noir-vkey")
		if err != nil {
			return fmt.Errorf("failed to create vKey file: %s", err)
		}
		_, err = f.Write(contract.ProgramId)
		if err != nil {
			return fmt.Errorf("failed to write vKey to file: %s", err)
		}
		outBytes, err := exec.Command("bun", "run", noirVerifierPath+"/verifier.ts", "--vKeyPath", "/tmp/noir-vkey", "--proofPath", "/tmp/noir-proof", "--outputPath", "/tmp/noir-output").Output()
		if err != nil {
			return fmt.Errorf("noir verifier failed on %s. Exit code: %s", msg.ContractName, err)
		}

		// Then parse data from the verified proof.
		err = json.Unmarshal(outBytes, &objmap)
		if err != nil {
			return fmt.Errorf("failed to unmarshal verifier output: %s", err)
		}

		//proofData = string(outBytes)
	} else if contract.Verifier == "cairo" {
		// Save proof to a local file
		err := os.WriteFile("/tmp/cairo-proof.json", msg.Proof, 0644)
		if err != nil {
			return fmt.Errorf("failed to write proof to file: %s", err)
		}

		outBytes, err := exec.Command(cairoVerifierPath, "verify", "/tmp/cairo-proof.json").Output()
		if err != nil {
			return fmt.Errorf("cairo verifier failed on %s. Exit code: %s", msg.ContractName, err)
		}

		// Then parse data from the verified proof.
		err = json.Unmarshal(outBytes, &objmap)
		if err != nil {
			return fmt.Errorf("failed to unmarshall verifier's output on %s. Exit code: %w", msg.ContractName, err)
		}

		//proofData = string(outBytes)
	} else if contract.Verifier == "gnark-groth16-te-BN254" {
		// Order: first parse the proof, verify data, and then verify proof (assuming fastest failure in that order)
		var proof gnark.Groth16Proof
		if err := json.Unmarshal(msg.Proof, &proof); err != nil {
			return fmt.Errorf("failed to unmarshal proof: %s", err)
		}

		if !bytes.Equal(proof.VerifyingKey, []byte(contract.ProgramId)) {
			return fmt.Errorf("verifying key does not match the known VK")
		}

		g16p, vk, witness, err := proof.ParseProof()
		if err != nil {
			return err
		}

		data, err := proof.ExtractData(witness)
		if err != nil {
			return err
		}

		*objmap = *data
		//proofData = base64.StdEncoding.EncodeToString(proof.PublicWitness)

		// Final step: actually check the proof here
		if err := groth16.Verify(g16p, vk, witness); err != nil {
			return fmt.Errorf("groth16 verification failed on %s: %w", msg.ContractName, err)
		}
	} else {
		return fmt.Errorf("unknown verifier %s", contract.Verifier)
	}
	return nil
}

func (ms msgServer) RegisterContract(goCtx context.Context, msg *zktx.MsgRegisterContract) (*zktx.MsgRegisterContractResponse, error) {

	if exists, err := ms.k.Contracts.Has(goCtx, msg.ContractName); err != nil || exists {
		return nil, fmt.Errorf("Contract with name {%s} already exists", msg.ContractName)
	}

	newContract := zktx.Contract{
		Verifier:    msg.Verifier,
		ProgramId:   msg.ProgramId,
		StateDigest: []byte(msg.StateDigest),
	}
	if err := ms.k.Contracts.Set(goCtx, msg.ContractName, newContract); err != nil {
		return nil, err
	}

	ctx := sdk.UnwrapSDKContext(goCtx)

	// Emit event by contract name for TX indexation
	if err := ctx.EventManager().EmitTypedEvent(&zktx.EventContractRegistered{ContractName: msg.ContractName}); err != nil {
		return nil, err
	}

	return &zktx.MsgRegisterContractResponse{}, nil
}
