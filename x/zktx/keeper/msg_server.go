package keeper

import (
	"bytes"
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"os/exec"
	"strings"

	"github.com/hyle/hyle/zktx"
	"github.com/hyle/hyle/zktx/keeper/gnark"

	"github.com/consensys/gnark/backend/groth16"
)

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

func (ms msgServer) ExecuteStateChange(ctx context.Context, msg *zktx.MsgExecuteStateChange) (*zktx.MsgExecuteStateChangeResponse, error) {
	hyleContext := zktx.HyleContext{
		Sender:    msg.HyleSender,
		Caller:    "",
		BlockTime: msg.BlockTime,
		BlockNb:   msg.BlockNb,
		TxHash:    []byte("TODO"),
	}

	// Check that the sender contract matches the contract name in the first message
	// Extract contract name from the last item in the sender
	// TODO (temp): ignore sender if we get passed nothing.
	if hyleContext.Sender != "" && len(msg.StateChanges) > 0 {
		paths := strings.Split(hyleContext.Sender, ".")
		if len(paths) < 2 || paths[len(paths)-1] != msg.StateChanges[0].ContractName {
			return nil, fmt.Errorf("invalid sender contract, expected %s, got %s", msg.StateChanges[0].ContractName, paths[len(paths)-1])
		}
	}

	for _, stateChange := range msg.StateChanges {
		if err := ms.actuallyExecuteStateChange(ctx, &hyleContext, stateChange); err != nil {
			return nil, err
		}
	}
	return &zktx.MsgExecuteStateChangeResponse{}, nil
}

func (ms msgServer) actuallyExecuteStateChange(ctx context.Context, hyleContext *zktx.HyleContext, msg *zktx.StateChange) error {
	contract, err := ms.k.Contracts.Get(ctx, msg.ContractName)
	if err != nil {
		return fmt.Errorf("invalid contract - no state is registered")
	}

	if !bytes.Equal(contract.StateDigest, msg.InitialState) {
		return fmt.Errorf("invalid initial state, expected %x, got %x", contract.StateDigest, msg.InitialState)
	}

	if contract.Verifier == "risczero" {
		// Save proof to a local file
		err = os.WriteFile("risc0-proof.json", msg.Proof, 0644)

		if err != nil {
			return fmt.Errorf("failed to write proof to file: %s", err)
		}

		verifierCmd := exec.Command(risczeroVerifierPath, contract.ProgramId, "risc0-proof.json", base64.StdEncoding.EncodeToString(msg.InitialState), base64.StdEncoding.EncodeToString(msg.FinalState))
		grepOut, _ := verifierCmd.StderrPipe()
		verifierCmd.Start()
		err = verifierCmd.Wait()

		if err != nil {
			grepBytes, _ := io.ReadAll(grepOut)
			fmt.Println(string(grepBytes))
			return fmt.Errorf("verifier failed. Exit code: %s", err)
		}

	} else if contract.Verifier == "sp1" {
		// Save proof to a local file
		err = os.WriteFile("sp1-proof.json", msg.Proof, 0644)

		if err != nil {
			return nil, fmt.Errorf("failed to write proof to file: %s", err)
		}

		// Save elf to a local file
		err = os.WriteFile("sp1-elf.bin", []byte(contract.ProgramId), 0644)

		if err != nil {
			return nil, fmt.Errorf("failed to write ELF to file: %s", err)
		}

		verifierCmd := exec.Command(sp1VerifierPath, "sp1-elf.bin", "sp1-proof.json", base64.StdEncoding.EncodeToString(msg.InitialState), base64.StdEncoding.EncodeToString(msg.FinalState))
		grepOut, _ := verifierCmd.StderrPipe()
		verifierCmd.Start()
		err = verifierCmd.Wait()

		if err != nil {
			grepBytes, _ := io.ReadAll(grepOut)
			fmt.Println(string(grepBytes))
			return nil, fmt.Errorf("verifier failed. Exit code: %s", err)
		}
	} else if contract.Verifier == "gnark-groth16-te-BN254" {
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

		// Ensure all compulsory witness data is present.
		err = proof.ValidateWitnessData(hyleContext, msg, witness)
		if err != nil {
			return err
		}

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
	contract.StateDigest = msg.FinalState
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

		verifierCmd := exec.Command(risczeroVerifierPath, contract.ProgramId, "risc0-proof.json")
		grepOut, _ := verifierCmd.StderrPipe()
		verifierCmd.Start()
		err = verifierCmd.Wait()

		if err != nil {
			grepBytes, _ := io.ReadAll(grepOut)
			fmt.Println(string(grepBytes))
			return nil, fmt.Errorf("verifier failed. Exit code: %s", err)
		}
	} else if contract.Verifier == "sp1" {
		// Save proof to a local file
		err = os.WriteFile("sp1-proof.json", msg.Proof, 0644)

		if err != nil {
			return nil, fmt.Errorf("failed to write proof to file: %s", err)
		}

		// Save elf to a local file
		err = os.WriteFile("sp1-elf.bin", []byte(contract.ProgramId), 0644)

		if err != nil {
			return nil, fmt.Errorf("failed to write ELF to file: %s", err)
		}

		verifierCmd := exec.Command(sp1VerifierPath, "sp1-elf.bin", "sp1-proof.json")
		grepOut, _ := verifierCmd.StderrPipe()
		verifierCmd.Start()
		err = verifierCmd.Wait()

		if err != nil {
			grepBytes, _ := io.ReadAll(grepOut)
			fmt.Println(string(grepBytes))
			return nil, fmt.Errorf("verifier failed. Exit code: %s", err)
		}

	} else if contract.Verifier == "gnark-groth16-te-BN254" {
		var proof gnark.Groth16Proof
		if err := json.Unmarshal(msg.Proof, &proof); err != nil {
			return nil, fmt.Errorf("failed to unmarshal proof: %s", err)
		}

		if !bytes.Equal(proof.VerifyingKey, []byte(contract.ProgramId)) {
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
