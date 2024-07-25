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
	"github.com/iden3/go-iden3-crypto/poseidon"

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

// NewMsgServerImpl returns an implementation of the module MsgServer interface.
func NewMsgServerImpl(keeper Keeper) zktx.MsgServer {
	// By default, assume the hylé repo shares a parent directory with the verifiers repo.
	// They'll still need to be compiled in release mode.
	// Noir expects bun to be installed.
	if risczeroVerifierPath == "" {
		risczeroVerifierPath = "../verifiers-for-hyle/target/release/risc0-verifier"
	}
	if sp1VerifierPath == "" {
		sp1VerifierPath = "../verifiers-for-hyle/target/release/sp1-verifier"
	}
	if noirVerifierPath == "" {
		noirVerifierPath = "../verifiers-for-hyle/noir-verifier"
	}
	if cairoVerifierPath == "" {
		cairoVerifierPath = "../verifiers-for-hyle/target/release/cairo-verifier"
	}
	return &msgServer{k: keeper}
}

func (ms msgServer) PublishPayloads(goCtx context.Context, msg *zktx.MsgPublishPayloads) (*zktx.MsgPublishPayloadsResponse, error) {
	ctx := sdk.UnwrapSDKContext(goCtx)
	for i, payload := range msg.Payloads {
		_, err := ms.k.Contracts.Get(ctx, payload.ContractName)
		if err != nil {
			return nil, fmt.Errorf("invalid contract - no state is registered")
		}
		// Compute txHash
		h := sha256.New()
		h.Write(ctx.TxBytes())
		txHash := h.Sum(nil)

		// Compute poseidon hash over payload.Data
		payloadHash, _ := poseidon.HashBytes(payload.Data)
		ms.k.ProvenPayload.Set(ctx, collections.Join(txHash, uint32(i)), zktx.PayloadMetadata{
			PayloadHash:   []byte(fmt.Sprintf("%x", payloadHash)),
			ContractName: payload.ContractName,
		})
	}
	// TODO fees
	return &zktx.MsgPublishPayloadsResponse{}, nil
}

func (ms msgServer) PublishPayloadProof(goCtx context.Context, msg *zktx.MsgPublishPayloadProof) (*zktx.MsgPublishPayloadProofResponse, error) {
	ctx := sdk.UnwrapSDKContext(goCtx)

	payload_metadata, err := ms.k.ProvenPayload.Get(ctx, collections.Join(msg.TxHash, msg.PayloadIndex))
	if err != nil {
		return nil, fmt.Errorf("no payload found for this txHash")
	}

	if !bytes.Equal(payload_metadata.PayloadHash, msg.PayloadHash) {
		return nil, fmt.Errorf("payload hash does not match the expected hash")
	}

	contract, err := ms.k.Contracts.Get(ctx, msg.ContractName)
	if err != nil {
		return nil, fmt.Errorf("invalid contract - no state is registered")
	}

	// TODO: add events back
	//var proofData string
	var objmap zktx.HyleOutput

	if err := extractProof(&objmap, &contract, msg); err != nil {
		return nil, err
	}

	if !bytes.Equal(contract.StateDigest, objmap.InitialState) {
		return nil, fmt.Errorf("verifier output does not match the expected initial state")
	}

	if !bytes.Equal(objmap.PayloadHash, payload_metadata.PayloadHash) {
		return nil, fmt.Errorf("proof is not related with correct payload hash")
	}

	payload_metadata.ContractName = msg.ContractName
	payload_metadata.NextState = objmap.NextState
	payload_metadata.Identity = objmap.Identity
	payload_metadata.Verified = true

	ms.k.ProvenPayload.Set(ctx, collections.Join(msg.TxHash, msg.PayloadIndex), payload_metadata)

	err = ms.maybeSettleTx(ctx, msg.TxHash)
	if err != nil {
		return nil, err
	}

	/*
		// Emit event by contract name for TX indexation
		if err := ctx.EventManager().EmitTypedEvent(&zktx.EventStateChange{ContractName: msg.ContractName, ProofData: proofData}); err != nil {
			return err
		}
	*/
	return &zktx.MsgPublishPayloadProofResponse{}, nil
}

func (ms msgServer) maybeSettleTx(ctx sdk.Context, txHash []byte) error {
	first_payload, err := ms.k.ProvenPayload.Get(ctx, collections.Join(txHash, uint32(0)))
	if err != nil {
		return fmt.Errorf("no payloads found for this tx")
	}
	expected_identity := first_payload.Identity
	if expected_identity != "" {
		// check that this matches the contract name
		paths := strings.Split(first_payload.Identity, ".")
		if len(paths) < 2 || paths[len(paths)-1] != first_payload.ContractName {
			return fmt.Errorf("invalid identity contract, expected '%s', got '%s'", first_payload.ContractName, paths[len(paths)-1])
		}

		for i := 1; ; i++ {
			payload_metadata, err := ms.k.ProvenPayload.Get(ctx, collections.Join(txHash, uint32(i)))
			if errors.Is(err, collections.ErrNotFound) {
				break
			}
			if payload_metadata.Identity != expected_identity || err != nil {
				return fmt.Errorf("payloads have different identities")
			}
		}
	}

	// Then update the state
	for i := 0; ; i++ {
		payload_metadata, err := ms.k.ProvenPayload.Get(ctx, collections.Join(txHash, uint32(i)))
		if errors.Is(err, collections.ErrNotFound) {
			return nil
		} else if err != nil {
			return err
		}
		contract, err := ms.k.Contracts.Get(ctx, payload_metadata.ContractName)
		if err != nil {
			return fmt.Errorf("invalid contract - no state is registered")
		}
		contract.StateDigest = payload_metadata.NextState
		if err := ms.k.Contracts.Set(ctx, payload_metadata.ContractName, contract); err != nil {
			return err
		}
		// This is safe - cosmos sdk will revert the whole thing if the TX fails
		ms.k.ProvenPayload.Remove(ctx, collections.Join(txHash, uint32(i)))
	}
}

func extractProof(objmap *zktx.HyleOutput, contract *zktx.Contract, msg *zktx.MsgPublishPayloadProof) error {
	if contract.Verifier == "risczero" {
		// Save proof to a local file
		err := os.WriteFile("/tmp/risc0-proof.json", msg.Proof, 0644)

		if err != nil {
			return fmt.Errorf("failed to write proof to file: %s", err)
		}

		b16ProgramId := hex.EncodeToString(contract.ProgramId)
		outBytes, err := exec.Command(risczeroVerifierPath, b16ProgramId, "/tmp/risc0-proof.json").Output()
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
		b64ProgramId := base64.StdEncoding.EncodeToString(contract.ProgramId)
		outBytes, err := exec.Command(sp1VerifierPath, b64ProgramId, "/tmp/sp1-proof.json").Output()
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
			panic(err)
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
