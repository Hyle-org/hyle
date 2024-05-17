package keeper

import (
	"bytes"
	"context"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"os/exec"
	"strings"

	"github.com/hyle/hyle/zktx"

	"github.com/consensys/gnark-crypto/ecc"
	"github.com/consensys/gnark-crypto/ecc/bn254/fr"
	tedwards "github.com/consensys/gnark-crypto/ecc/twistededwards"
	"github.com/consensys/gnark/backend/groth16"
	"github.com/consensys/gnark/backend/witness"
	"github.com/consensys/gnark/std/algebra/native/twistededwards"
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

type Groth16Proof struct {
	Proof         []byte `json:"proof"`
	VerifyingKey  []byte `json:"verifying_key"`
	PublicWitness []byte `json:"public_witness"`
}

func (proof *Groth16Proof) ParseProof() (groth16.Proof, groth16.VerifyingKey, witness.Witness, error) {
	proofReader := bytes.NewReader(proof.Proof)
	g16p := groth16.NewProof(ecc.BN254)
	if _, err := g16p.ReadFrom(proofReader); err != nil {
		return nil, nil, nil, fmt.Errorf("failed to parse groth16 proof: %s", err)
	}

	proofReader = bytes.NewReader(proof.VerifyingKey)
	vk := groth16.NewVerifyingKey(ecc.BN254)
	if _, err := vk.ReadFrom(proofReader); err != nil {
		return nil, nil, nil, fmt.Errorf("failed to parse groth16 vk: %w", err)
	}

	proofReader = bytes.NewReader(proof.PublicWitness)
	fid, _ := twistededwards.GetSnarkField(tedwards.BN254) // Note: handle the error if required
	witness, err := witness.New(fid)
	if err != nil {
		return nil, nil, nil, fmt.Errorf("failed to initialize groth16 witness: %w", err)
	}

	if _, err := witness.ReadFrom(proofReader); err != nil {
		return nil, nil, nil, fmt.Errorf("failed to parse groth16 witness: %w", err)
	}

	return g16p, vk, witness, nil
}

func (ms msgServer) ExecuteStateChange(ctx context.Context, msg *zktx.MsgExecuteStateChange) (*zktx.MsgExecuteStateChangeResponse, error) {
	contract, err := ms.k.Contracts.Get(ctx, msg.ContractName)
	if err != nil {
		return nil, fmt.Errorf("invalid contract - no state is registered")
	}

	if !bytes.Equal(contract.StateDigest, msg.InitialState) {
		return nil, fmt.Errorf("invalid initial state, expected %x, got %x", contract.StateDigest, msg.InitialState)
	}

	if contract.Verifier == "risczero" {
		// Save proof to a local file
		err = os.WriteFile("risc0-proof.json", msg.Proof, 0644)

		if err != nil {
			return nil, fmt.Errorf("failed to write proof to file: %s", err)
		}

		verifierCmd := exec.Command(risczeroVerifierPath, contract.ProgramId, "risc0-proof.json", base64.StdEncoding.EncodeToString(msg.InitialState), base64.StdEncoding.EncodeToString(msg.FinalState))
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
		var proof Groth16Proof
		if err := json.Unmarshal(msg.Proof, &proof); err != nil {
			return nil, fmt.Errorf("failed to unmarshal proof: %s", err)
		}

		if hex.EncodeToString(proof.VerifyingKey) != contract.ProgramId {
			return nil, fmt.Errorf("verifying key does not match the known VK")
		}

		g16p, vk, witness, err := proof.ParseProof()
		if err != nil {
			return nil, err
		}

		// Check payload version identifier
		pubWitVector, ok := witness.Vector().(fr.Vector)
		if !ok {
			return nil, fmt.Errorf("failed to cast witness vector to fr.Vector")
		} else if pubWitVector[0] != fr.NewElement(1) {
			return nil, fmt.Errorf("invalid version identifier %s, expected 1", pubWitVector[0].Text(10))
		}

		// Extracting witness data is quite annoying and serialization formats vary.
		// The approach in version one is straight binary serialization comparison.
		// This is brittle, but it works for now.
		// Expected format of the witness, serialized big-endian:
		// u32(nb public inputs) | u32(nb private inputs (must be 0 as this is the public witness))
		// u32(nb vector items) | 32 bytes per field element...

		// First let's check lengths to avoid panics
		if len(proof.PublicWitness) < 12+32+len(msg.InitialState)+len(msg.FinalState) {
			return nil, fmt.Errorf("invalid witness length, expected at least %d bytes, got %d", 12+32+len(msg.InitialState)+len(msg.FinalState), len(proof.PublicWitness))
		}

		// First compare the initial state, skipping over the lengths and version identifier
		witnessInitialState := proof.PublicWitness[12+32 : 12+32+len(msg.InitialState)]
		if !bytes.Equal(witnessInitialState, msg.InitialState) {
			return nil, fmt.Errorf("incorrect initial state, expected %x, got %x", msg.InitialState, witnessInitialState)
		}
		// Then the final state
		witnessFinalState := proof.PublicWitness[12+32+len(msg.InitialState) : 12+32+len(msg.InitialState)+len(msg.FinalState)]
		if !bytes.Equal(witnessFinalState, msg.FinalState) {
			return nil, fmt.Errorf("incorrect final state, expected %x, got %x", msg.FinalState, witnessFinalState)
		}

		// Final step: actually check the proof here
		if err := groth16.Verify(g16p, vk, witness); err != nil {
			return nil, fmt.Errorf("groth16 verification failed: %w", err)
		}
	} else {
		return nil, fmt.Errorf("unknown verifier %s", contract.Verifier)
	}

	// Update contract
	contract.StateDigest = msg.FinalState
	if err := ms.k.Contracts.Set(ctx, msg.ContractName, contract); err != nil {
		return nil, err
	}

	return &zktx.MsgExecuteStateChangeResponse{}, nil
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
		var proof Groth16Proof
		if err := json.Unmarshal(msg.Proof, &proof); err != nil {
			return nil, fmt.Errorf("failed to unmarshal proof: %s", err)
		}

		if hex.EncodeToString(proof.VerifyingKey) != contract.ProgramId {
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
