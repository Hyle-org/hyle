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
	"math/big"
	"os"
	"os/exec"
	"strings"

	"cosmossdk.io/collections"
	sdk "github.com/cosmos/cosmos-sdk/types"

	"github.com/hyle-org/hyle/x/zktx"
	"github.com/hyle-org/hyle/x/zktx/keeper/gnark"

	"github.com/consensys/gnark-crypto/ecc/stark-curve/fp"
	pedersenhash "github.com/consensys/gnark-crypto/ecc/stark-curve/pedersen-hash"
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

var txTimeout = int64(100)

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

// ParseCairoPayload parses cairo payload
func ParseCairoPayload(payload []byte) []string {
	elements := strings.Split(strings.Trim(string(payload), "[]"), " ")
	var cairoPayload []string
	for _, elem := range elements {
		elem = strings.TrimSpace(elem)
		if elem != "" {
			cairoPayload = append(cairoPayload, elem)
		}
	}
	return cairoPayload
}

// HashCairoPayload hashes cairo payload
func HashCairoPayload(cairoPayload []string) (*big.Int, error) {
	var inputsElements []*fp.Element
	for i := 0; i < len(cairoPayload); i++ {
		elem, err := new(fp.Element).SetString(cairoPayload[i])
		if err != nil {
			return nil, err
		}

		inputsElements = append(inputsElements, elem)
	}
	pedersenHashedData := pedersenhash.PedersenArray(inputsElements...)
	return pedersenHashedData.BigInt(new(big.Int)), nil
}

func computePayloadHash(verifier string, payloadData []byte) ([]byte, error) {
	if verifier == "cairo" {
		// Compute pedersen hash over payload.Data
		cairoPayload := ParseCairoPayload(payloadData)
		payloadHash, err := HashCairoPayload(cairoPayload)
		if err != nil {
			return nil, err
		}
		return payloadHash.Bytes(), nil

	} else if verifier == "noir" {
		// TODO: hash payloadData for noir
		// ATM we use 0 as payloadHash for convenience
		// Hence it is !mandatory! for the noir code to use 0 as payloadHash
		return make([]byte, 4), nil
	} else if verifier == "risczero" {
		return payload_data, nil
	}
	return nil, fmt.Errorf("failed to hash payload: hash function not implemented for verifier %s", verifier)
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

		// Compute payload hash
		payloadHash, err := computePayloadHash(contract.Verifier, payload.Data)
		if err != nil {
			return nil, err
		}

		// Store enough metadata to check proofs later.
		err = ms.k.ProvenPayload.Set(ctx, collections.Join(txHash, uint32(i)), zktx.PayloadMetadata{
			PayloadHash:  payloadHash,
			ContractName: payload.ContractName,
			Identity:     msg.Identity,
		})
		if err != nil {
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

	if !bytes.Equal(contract.StateDigest, objmap.InitialState) {
		return nil, fmt.Errorf("verifier output does not match the expected initial state")
	}

	if !bytes.Equal(objmap.PayloadHash, payloadMetadata.PayloadHash) {
		return nil, fmt.Errorf("proof is not related with correct payload hash")
	}

	if payloadMetadata.Identity != "" && objmap.Identity != payloadMetadata.Identity {
		return nil, fmt.Errorf("proof is not for the correct identity")
	}

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

// MaybeSettleTx zef
func MaybeSettleTx(k Keeper, ctx sdk.Context, txHash []byte) error {
	// Check if all payloads have been verified
	success := true
	fullyVerified := true
	for i := 0; ; i++ {
		payloadMetadata, err := k.ProvenPayload.Get(ctx, collections.Join(txHash, uint32(i)))
		if errors.Is(err, collections.ErrNotFound) {
			break
		}
		if !payloadMetadata.Verified {
			fullyVerified = false
		} else if !payloadMetadata.Success {
			// TODO: figure out side effects?
			// If any payload fail, the whole TX can be rejected
			success = false
			break
		}
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
		if success {
			contract, err := k.Contracts.Get(ctx, payloadMetadata.ContractName)
			if err != nil {
				return fmt.Errorf("invalid contract - no state is registered")
			}
			contract.StateDigest = payloadMetadata.NextState
			if err := k.Contracts.Set(ctx, payloadMetadata.ContractName, contract); err != nil {
				return err
			}
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
		err := os.WriteFile("/tmp/noir-proof.json", msg.Proof, 0644)
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
		outBytes, err := exec.Command("bun", "run", noirVerifierPath+"/verifier.ts", "--vKeyPath", "/tmp/noir-vkey", "--proofPath", "/tmp/noir-proof.json").Output()
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
