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

	"github.com/hyle-org/hyle/x/zktx"
	"github.com/hyle-org/hyle/x/zktx/keeper/gnark"

	"github.com/consensys/gnark/backend/groth16"
)

// For clarity, split from ValidateProofData
func SetContextIfNeeded(proofData zktx.HyleOutput, hyleContext *zktx.HyleContext) error {
	// Only do this on the first call
	if hyleContext.Caller != "" {
		return nil
	}
	hyleContext.Sender = proofData.Sender
	return nil
}

// TODO check block number, block time and tx hash
func ValidateProofData(proofData zktx.HyleOutput, initialState []byte, hyleContext *zktx.HyleContext) error {
	if !bytes.Equal(proofData.InitialState, initialState) {
		return fmt.Errorf("verifier output does not match the expected initial state")
	}
	// Assume that if the proof has an empty sender/caller, it's "free for all"
	if proofData.Sender != "" && proofData.Sender != hyleContext.Sender {
		return fmt.Errorf("verifier output does not match the expected sender")
	}
	if proofData.Caller != "" && proofData.Caller != hyleContext.Caller {
		return fmt.Errorf("verifier output does not match the expected caller")
	}
	return nil
}

type msgServer struct {
	k Keeper
}

var _ zktx.MsgServer = msgServer{}

var risczeroVerifierPath = os.Getenv("RISCZERO_VERIFIER_PATH")
var sp1VerifierPath = os.Getenv("SP1_VERIFIER_PATH")

// NewMsgServerImpl returns an implementation of the module MsgServer interface.
func NewMsgServerImpl(keeper Keeper) zktx.MsgServer {
	if risczeroVerifierPath == "" {
		risczeroVerifierPath = "/hyle/risc0-verifier"
	}
	if sp1VerifierPath == "" {
		sp1VerifierPath = "/hyle/sp1-verifier"
	}

	return &msgServer{k: keeper}
}

func (ms msgServer) ExecuteStateChanges(ctx context.Context, msg *zktx.MsgExecuteStateChanges) (*zktx.MsgExecuteStateChangesResponse, error) {
	if len(msg.StateChanges) == 0 {
		return &zktx.MsgExecuteStateChangesResponse{}, nil
	}

	// Initialize context with unknown data at this point
	hyleContext := zktx.HyleContext{
		Sender:    "",
		Caller:    "",
		BlockTime: 0,
		BlockNb:   0,
		TxHash:    []byte("TODO"),
	}

	for i, stateChange := range msg.StateChanges {
		if err := ms.actuallyExecuteStateChange(ctx, &hyleContext, stateChange); err != nil {
			return nil, err
		}
		if i == 0 && hyleContext.Sender != "" {
			// Check sender matches contract name
			paths := strings.Split(hyleContext.Sender, ".")
			if len(paths) < 2 || paths[len(paths)-1] != stateChange.ContractName {
				return nil, fmt.Errorf("invalid sender contract, expected '%s', got '%s'", stateChange.ContractName, paths[len(paths)-1])
			}
		}
	}

	return &zktx.MsgExecuteStateChangesResponse{}, nil
}

func (ms msgServer) actuallyExecuteStateChange(ctx context.Context, hyleContext *zktx.HyleContext, msg *zktx.StateChange) error {
	contract, err := ms.k.Contracts.Get(ctx, msg.ContractName)
	if err != nil {
		return fmt.Errorf("invalid contract - no state is registered")
	}

	var finalStateDigest []byte

	if contract.Verifier == "risczero" {
		// Save proof to a local file
		err = os.WriteFile("risc0-proof.json", msg.Proof, 0644)

		if err != nil {
			return fmt.Errorf("failed to write proof to file: %s", err)
		}

		b16ProgramId := hex.EncodeToString(contract.ProgramId)
		outBytes, err := exec.Command(risczeroVerifierPath, b16ProgramId, "risc0-proof.json").Output()
		if err != nil {
			return fmt.Errorf("verifier failed. Exit code: %s", err)
		}
		// Then parse data from the verified proof.
		var objmap zktx.HyleOutput
		err = json.Unmarshal(outBytes, &objmap)
		if err != nil {
			return fmt.Errorf("failed to unmarshal verifier output: %s", err)
		}

		SetContextIfNeeded(objmap, hyleContext)
		if err = ValidateProofData(objmap, contract.StateDigest, hyleContext); err != nil {
			return err
		}

		finalStateDigest = objmap.NextState
	} else if contract.Verifier == "sp1" {
		// Save proof to a local file
		err = os.WriteFile("sp1-proof.json", msg.Proof, 0644)

		if err != nil {
			return fmt.Errorf("failed to write proof to file: %s", err)
		}
		b64ProgramId := base64.StdEncoding.EncodeToString(contract.ProgramId)
		outBytes, err := exec.Command(sp1VerifierPath, b64ProgramId, "sp1-proof.json").Output()
		if err != nil {
			return fmt.Errorf("verifier failed. Exit code: %s", err)
		}
		// Then parse data from the verified proof.
		var objmap zktx.HyleOutput
		err = json.Unmarshal(outBytes, &objmap)
		if err != nil {
			return fmt.Errorf("failed to unmarshal verifier output: %s", err)
		}

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

	// Update the caller for future state changes
	hyleContext.Caller = msg.ContractName

	// TODO: check block time / number / TX Hash

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
		err = os.WriteFile("risc0-proof.json", msg.Proof, 0644)

		if err != nil {
			return nil, fmt.Errorf("failed to write proof to file: %s", err)
		}

		b16ProgramId := hex.EncodeToString(contract.ProgramId)
		_, err := exec.Command(risczeroVerifierPath, b16ProgramId, "risc0-proof.json").Output()
		if err != nil {
			return nil, fmt.Errorf("verifier failed. Exit code: %s", err)
		}
	} else if contract.Verifier == "sp1" {
		// Save proof to a local file
		err = os.WriteFile("sp1-proof.json", msg.Proof, 0644)

		if err != nil {
			return nil, fmt.Errorf("failed to write proof to file: %s", err)
		}
		b64ProgramId := base64.StdEncoding.EncodeToString(contract.ProgramId)
		_, err := exec.Command(sp1VerifierPath, b64ProgramId, "sp1-proof.json").Output()
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

func (ms msgServer) RegisterContract(ctx context.Context, msg *zktx.MsgRegisterContract) (*zktx.MsgRegisterContractResponse, error) {

	if exists, err := ms.k.Contracts.Has(ctx, msg.ContractName); err != nil || exists {
		return nil, fmt.Errorf("Contract with name {%s} already exists", msg.ContractName)
	}

	newContract := zktx.Contract{
		Verifier:    msg.Verifier,
		ProgramId:   msg.ProgramId,
		StateDigest: []byte(msg.StateDigest),
	}
	if err := ms.k.Contracts.Set(ctx, msg.ContractName, newContract); err != nil {
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
