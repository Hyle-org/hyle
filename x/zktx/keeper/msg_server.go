package keeper

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"os/exec"
	"strings"

	"github.com/hyle/hyle/zktx"

	"github.com/consensys/gnark-crypto/ecc"
	tedwards "github.com/consensys/gnark-crypto/ecc/twistededwards"
	"github.com/consensys/gnark/backend/groth16"
	"github.com/consensys/gnark/backend/witness"
	"github.com/consensys/gnark/std/algebra/native/twistededwards"

	"github.com/consensys/gnark/frontend"

	"github.com/consensys/gnark/std/math/emulated"
	circuitecdsa "github.com/consensys/gnark/std/signature/ecdsa"
)

type msgServer struct {
	k Keeper
}

var _ zktx.MsgServer = msgServer{}

var risczeroVerifierPath = os.Getenv("RISCZERO_VERIFIER_PATH")

// NewMsgServerImpl returns an implementation of the module MsgServer interface.
func NewMsgServerImpl(keeper Keeper) zktx.MsgServer {
	if risczeroVerifierPath == "" {
		risczeroVerifierPath = "/hyle/risc-zero/verifier"
	}

	return &msgServer{k: keeper}
}

type Groth16Proof struct {
	Proof         []byte `json:"proof",string`
	VerifyingKey  []byte `json:"verifying_key",string`
	PublicWitness []byte `json:"public_witness",string`
}

type verifiableCircuitAPI struct {
	Input  frontend.Variable `gnark:",public"`
	Output frontend.Variable `gnark:",public"`
}

func (c *verifiableCircuitAPI) Define(api frontend.API) error {
	return nil
}

type verifiableEcdsaAPI[T, S emulated.FieldParams] struct {
	PublicKey circuitecdsa.PublicKey[T, S] `gnark:",public"`
}

func (c *verifiableEcdsaAPI[T, S]) Define(api frontend.API) error {
	return nil
}

func (ms msgServer) ExecuteStateChange(ctx context.Context, msg *zktx.MsgExecuteStateChange) (*zktx.MsgExecuteStateChangeResponse, error) {
	contract, err := ms.k.Contracts.Get(ctx, msg.ContractName)
	if err != nil {
		return nil, fmt.Errorf("invalid contract - no state is registered")
	}

	if !bytes.Equal(contract.StateDigest, msg.InitialState) {
		return nil, fmt.Errorf("invalid initial contract, expected %x, got %x", contract.StateDigest, msg.InitialState)
	}

	if contract.Verifier == "risczero" {
		// Save proof to a local file
		err = os.WriteFile("proof.json", msg.Proof, 0644)

		if err != nil {
			return nil, fmt.Errorf("failed to write proof to file: %s", err)
		}

		verifierCmd := exec.Command(risczeroVerifierPath, contract.ProgramId, "proof.json", string(msg.InitialState), string(msg.FinalState))
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

		if !bytes.Equal(proof.VerifyingKey, []byte(contract.ProgramId)) {
			return nil, fmt.Errorf("verifying key does not match the known VK")
		}

		proofReader := bytes.NewReader(proof.Proof)
		g16p := groth16.NewProof(ecc.BN254)
		if _, err = g16p.ReadFrom(proofReader); err != nil {
			return nil, fmt.Errorf("failed to parse groth16 proof: %s", err)
		}

		proofReader = bytes.NewReader(proof.VerifyingKey)
		vk := groth16.NewVerifyingKey(ecc.BN254)
		if _, err := vk.ReadFrom(proofReader); err != nil {
			return nil, fmt.Errorf("failed to parse groth16 vk: %w", err)
		}

		proofReader = bytes.NewReader(proof.PublicWitness)
		fid, _ := twistededwards.GetSnarkField(tedwards.BN254) // Note: handle the error if required
		witness, err := witness.New(fid)
		if err != nil {
			return nil, fmt.Errorf("failed to initialize groth16 witness: %w", err)
		}

		if _, err := witness.ReadFrom(proofReader); err != nil {
			return nil, fmt.Errorf("failed to parse groth16 witness: %w", err)
		}

		// For now the fastest way I found to verify is to serialize my own input and make sure it matches in binary rep (horrible)
		witness_circuit := verifiableCircuitAPI{
			Input:  msg.InitialState,
			Output: msg.FinalState,
		}
		payload_witness, err := frontend.NewWitness(&witness_circuit, ecc.BN254.ScalarField())
		if err != nil {
			return nil, fmt.Errorf("failed to generate payload_witness: %w", err)
		}

		var payloadWitness bytes.Buffer
		payload_witness.WriteTo(&payloadWitness)
		if !bytes.Equal(proof.PublicWitness, payloadWitness.Bytes()) {
			return nil, fmt.Errorf("publicWitness and payload_witness do not match")
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
		err = os.WriteFile("proof.json", msg.Proof, 0644)

		if err != nil {
			return nil, fmt.Errorf("failed to write proof to file: %s", err)
		}

		verifierCmd := exec.Command(risczeroVerifierPath, contract.ProgramId, "proof.json")
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

		if !bytes.Equal(proof.VerifyingKey, []byte(contract.ProgramId)) {
			return nil, fmt.Errorf("verifying key does not match the known VK")
		}

		proofReader := bytes.NewReader(proof.Proof)
		g16p := groth16.NewProof(ecc.BN254)
		if _, err = g16p.ReadFrom(proofReader); err != nil {
			return nil, fmt.Errorf("failed to parse groth16 proof: %s", err)
		}

		proofReader = bytes.NewReader(proof.VerifyingKey)
		vk := groth16.NewVerifyingKey(ecc.BN254)
		if _, err := vk.ReadFrom(proofReader); err != nil {
			return nil, fmt.Errorf("failed to parse groth16 vk: %w", err)
		}

		proofReader = bytes.NewReader(proof.PublicWitness)
		fid, _ := twistededwards.GetSnarkField(tedwards.BN254) // Note: handle the error if required
		witness, err := witness.New(fid)
		if err != nil {
			return nil, fmt.Errorf("failed to initialize groth16 witness: %w", err)
		}

		if _, err := witness.ReadFrom(proofReader); err != nil {
			return nil, fmt.Errorf("failed to parse groth16 witness: %w", err)
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
