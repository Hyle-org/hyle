package keeper

import (
	"bytes"
	"context"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"strings"

	sdk "github.com/cosmos/cosmos-sdk/types"

	"github.com/hyle-org/hyle/x/zktx"
	"github.com/hyle-org/hyle/x/zktx/keeper/gnark"

	"github.com/consensys/gnark/backend/groth16"
)

// For clarity, split from ValidateProofData
func SetContextIfNeeded(proofData zktx.HyleOutput, hyleContext *zktx.HyleContext) error {
	if hyleContext.Identity == "" {
		hyleContext.Identity = proofData.Identity
	}
	return nil
}

// TODO check tx hash
func ValidateProofData(proofData zktx.HyleOutput, initialState []byte, hyleContext *zktx.HyleContext) error {
	if !bytes.Equal(proofData.InitialState, initialState) {
		return fmt.Errorf("verifier output does not match the expected initial state, got %x, expected %x", proofData.InitialState, initialState)
	}
	// Assume that if the proof has an empty Identity, it's "free for all"
	if proofData.Identity != "" && proofData.Identity != hyleContext.Identity {
		return fmt.Errorf("verifier output does not match the expected identity")
	}
	return nil
}

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

func (ms msgServer) ExecuteStateChanges(goCtx context.Context, msg *zktx.MsgExecuteStateChanges) (*zktx.MsgExecuteStateChangesResponse, error) {
	if len(msg.StateChanges) == 0 {
		return &zktx.MsgExecuteStateChangesResponse{}, nil
	}

	ctx := sdk.UnwrapSDKContext(goCtx)

	// Initialize context with unknown data at this point
	hyleContext := zktx.HyleContext{
		Identity: "",
		TxHash:   []byte("TODO"),
	}

	for i, stateChange := range msg.StateChanges {
		if err := ms.actuallyExecuteStateChange(ctx, &hyleContext, stateChange); err != nil {
			return nil, err
		}
		if i == 0 && hyleContext.Identity != "" {
			// Check identity matches contract name
			paths := strings.Split(hyleContext.Identity, ".")
			if len(paths) < 2 || paths[len(paths)-1] != stateChange.ContractName {
				return nil, fmt.Errorf("invalid identity contract, expected '%s', got '%s'", stateChange.ContractName, paths[len(paths)-1])
			}
		}
	}

	return &zktx.MsgExecuteStateChangesResponse{}, nil
}

func (ms msgServer) actuallyExecuteStateChange(ctx sdk.Context, hyleContext *zktx.HyleContext, msg *zktx.StateChange) error {
	contract, err := ms.k.Contracts.Get(ctx, msg.ContractName)
	if err != nil {
		return fmt.Errorf("invalid contract - no state is registered")
	}

	var finalStateDigest []byte

	var proofData string

	if contract.Verifier == "risczero" {
		// Save proof to a local file
		err = os.WriteFile("/tmp/risc0-proof.json", msg.Proof, 0644)

		if err != nil {
			return fmt.Errorf("failed to write proof to file: %s", err)
		}

		b16ProgramId := hex.EncodeToString(contract.ProgramId)
		outBytes, err := exec.Command(risczeroVerifierPath, b16ProgramId, "/tmp/risc0-proof.json").Output()
		if err != nil {
			return fmt.Errorf("verifier failed. Exit code: %s", err)
		}
		// Then parse data from the verified proof.
		var objmap zktx.HyleOutput
		err = json.Unmarshal(outBytes, &objmap)
		if err != nil {
			return fmt.Errorf("failed to unmarshal verifier output: %s", err)
		}

		proofData = string(outBytes)

		SetContextIfNeeded(objmap, hyleContext)
		if err = ValidateProofData(objmap, contract.StateDigest, hyleContext); err != nil {
			return err
		}

		finalStateDigest = objmap.NextState
	} else if contract.Verifier == "sp1" {
		// Save proof to a local file
		err = os.WriteFile("/tmp/sp1-proof.json", msg.Proof, 0644)

		if err != nil {
			return fmt.Errorf("failed to write proof to file: %s", err)
		}
		b64ProgramId := base64.StdEncoding.EncodeToString(contract.ProgramId)
		outBytes, err := exec.Command(sp1VerifierPath, b64ProgramId, "/tmp/sp1-proof.json").Output()
		if err != nil {
			return fmt.Errorf("verifier failed. Exit code: %s", err)
		}
		// Then parse data from the verified proof.
		var objmap zktx.HyleOutput
		err = json.Unmarshal(outBytes, &objmap)
		if err != nil {
			return fmt.Errorf("failed to unmarshal verifier output: %s", err)
		}

		proofData = string(outBytes)

		SetContextIfNeeded(objmap, hyleContext)
		if err = ValidateProofData(objmap, contract.StateDigest, hyleContext); err != nil {
			return err
		}

		finalStateDigest = objmap.NextState
	} else if contract.Verifier == "noir" {
		// Save proof to a local file
		err = os.WriteFile("/tmp/noir-proof.json", msg.Proof, 0644)
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
			return fmt.Errorf("verifier failed. Exit code: %s", err)
		}

		// Then parse data from the verified proof.
		var objmap zktx.HyleOutput
		err = json.Unmarshal(outBytes, &objmap)
		if err != nil {
			return fmt.Errorf("failed to unmarshal verifier output: %s", err)
		}

		proofData = string(outBytes)

		SetContextIfNeeded(objmap, hyleContext)
		if err = ValidateProofData(objmap, contract.StateDigest, hyleContext); err != nil {
			return err
		}

		finalStateDigest = objmap.NextState
	} else if contract.Verifier == "cairo" {
		// Save proof to a local file
		err = os.WriteFile("/tmp/cairo-proof.json", msg.Proof, 0644)
		if err != nil {
			return fmt.Errorf("failed to write proof to file: %s", err)
		}

		outBytes, err := exec.Command(cairoVerifierPath, "verify", "/tmp/cairo-proof.json").Output()
		if err != nil {
			return fmt.Errorf("verifier failed. Exit code: %s", err)
		}

		// Then parse data from the verified proof.
		var objmap zktx.HyleOutput
		err = json.Unmarshal(outBytes, &objmap)
		if err != nil {
			panic(err)
		}

		proofData = string(outBytes)

		SetContextIfNeeded(objmap, hyleContext)
		if err = ValidateProofData(objmap, contract.StateDigest, hyleContext); err != nil {
			return err
		}

		finalStateDigest = objmap.NextState
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

		proofData = base64.StdEncoding.EncodeToString(proof.PublicWitness)

		SetContextIfNeeded(*data, hyleContext)
		if err = ValidateProofData(*data, contract.StateDigest, hyleContext); err != nil {
			return err
		}

		finalStateDigest = data.NextState

		// Final step: actually check the proof here
		if err := groth16.Verify(g16p, vk, witness); err != nil {
			return fmt.Errorf("groth16 verification failed: %w", err)
		}
	} else {
		return fmt.Errorf("unknown verifier %s", contract.Verifier)
	}

	// TODO: validate tx Hash

	// Emit event by contract name for TX indexation
	if err := ctx.EventManager().EmitTypedEvent(&zktx.EventStateChange{ContractName: msg.ContractName, ProofData: proofData}); err != nil {
		return err
	}

	// Update contract
	contract.StateDigest = finalStateDigest
	if err := ms.k.Contracts.Set(ctx, msg.ContractName, contract); err != nil {
		return err
	}

	return nil
}

func (ms msgServer) VerifyProof(ctx context.Context, msg *zktx.MsgVerifyProof) (*zktx.MsgVerifyProofResponse, error) {
	contract, err := ms.k.Contracts.Get(ctx, msg.ContractName)
	if err != nil {
		return nil, fmt.Errorf("invalid contract - no state is registered")
	}

	if contract.Verifier == "risczero" {
		// Save proof to a local file
		err = os.WriteFile("/tmp/risc0-proof.json", msg.Proof, 0644)

		if err != nil {
			return nil, fmt.Errorf("failed to write proof to file: %s", err)
		}

		b16ProgramId := hex.EncodeToString(contract.ProgramId)
		_, err := exec.Command(risczeroVerifierPath, b16ProgramId, "/tmp/risc0-proof.json").Output()
		if err != nil {
			return nil, fmt.Errorf("verifier failed. Exit code: %s", err)
		}
	} else if contract.Verifier == "sp1" {
		// Save proof to a local file
		err = os.WriteFile("/tmp/sp1-proof.json", msg.Proof, 0644)

		if err != nil {
			return nil, fmt.Errorf("failed to write proof to file: %s", err)
		}
		b64ProgramId := base64.StdEncoding.EncodeToString(contract.ProgramId)
		_, err := exec.Command(sp1VerifierPath, b64ProgramId, "/tmp/sp1-proof.json").Output()
		if err != nil {
			return nil, fmt.Errorf("verifier failed. Exit code: %s", err)
		}
	} else if contract.Verifier == "noir" {
		// Save proof to a local file
		err = os.WriteFile("/tmp/noir-proof.json", msg.Proof, 0644)
		if err != nil {
			return nil, fmt.Errorf("failed to write proof to file: %s", err)
		}
		// Save vKey to a local file
		err = os.WriteFile("/tmp/noir-vkey.b64", contract.ProgramId, 0644)
		if err != nil {
			return nil, fmt.Errorf("failed to write vKey to file: %s", err)
		}

		_, err := exec.Command("bun", "run", noirVerifierPath, "--vKeyPath", "/tmp/noir-vkey.b64", "--proofPath", "/tmp/noir-proof.json").Output()
		if err != nil {
			return nil, fmt.Errorf("verifier failed. Exit code: %s", err)
		}
	} else if contract.Verifier == "cairo" {
		// Save proof to a local file
		err = os.WriteFile("/tmp/cairo-proof.json", msg.Proof, 0644)
		if err != nil {
			return nil, fmt.Errorf("failed to write proof to file: %s", err)
		}

		_, err := exec.Command(cairoVerifierPath, "verify", "/tmp/cairo-proof.json").Output()
		if err != nil {
			return nil, fmt.Errorf("verifier failed. Exit code: %s", err)
		}

	} else if contract.Verifier == "gnark-groth16-te-BN254" {
		var proof gnark.Groth16Proof
		if err := json.Unmarshal(msg.Proof, &proof); err != nil {
			return nil, fmt.Errorf("failed to unmarshal proof: %s", err)
		}

		if !bytes.Equal(proof.VerifyingKey, contract.ProgramId) {
			return nil, fmt.Errorf("verifying key does not match the known VK")
		}

		g16p, vk, witness, err := proof.ParseProof()
		if err != nil {
			return nil, err
		}

		if err := groth16.Verify(g16p, vk, witness); err != nil {
			return nil, fmt.Errorf("groth16 verification failed: %w", err)
		}
	} else {
		return nil, fmt.Errorf("unknown verifier %s", contract.Verifier)
	}

	return &zktx.MsgVerifyProofResponse{}, nil
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

///// Stuff from the default go project in the cosmos sdk minichain

// UpdateParams params is defining the handler for the MsgUpdateParams message.
func (ms msgServer) UpdateParams(ctx context.Context, msg *zktx.MsgUpdateParams) (*zktx.MsgUpdateParamsResponse, error) {
	if _, err := ms.k.addressCodec.StringToBytes(msg.Authority); err != nil {
		return nil, fmt.Errorf("invalid authority address: %w", err)
	}

	if authority := ms.k.GetAuthority(); !strings.EqualFold(msg.Authority, authority) {
		return nil, fmt.Errorf("unauthorized, authority does not match the module's authority: got %s, want %s", msg.Authority, authority)
	}

	if err := msg.Params.Validate(); err != nil {
		return nil, err
	}

	if err := ms.k.Params.Set(ctx, msg.Params); err != nil {
		return nil, err
	}

	return &zktx.MsgUpdateParamsResponse{}, nil
}
