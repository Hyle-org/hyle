package keeper

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"strings"

	"cosmossdk.io/collections"
	"github.com/hyle/hyle/zktx"

	"github.com/consensys/gnark-crypto/ecc"
	tedwards "github.com/consensys/gnark-crypto/ecc/twistededwards"
	"github.com/consensys/gnark/backend/groth16"
	"github.com/consensys/gnark/backend/witness"
	"github.com/consensys/gnark/std/algebra/native/twistededwards"
)

type msgServer struct {
	k Keeper
}

var _ zktx.MsgServer = msgServer{}

// NewMsgServerImpl returns an implementation of the module MsgServer interface.
func NewMsgServerImpl(keeper Keeper) zktx.MsgServer {
	return &msgServer{k: keeper}
}

type Groth16Proof struct {
	Proof         []byte `json:"proof",string`
	VerifyingKey  []byte `json:"verifying_key",string`
	PublicWitness []byte `json:"public_witness",string`
}

func (ms msgServer) ExecuteStateChange(ctx context.Context, msg *zktx.MsgExecuteStateChange) (*zktx.MsgExecuteStateChangeResponse, error) {
	contract, err := ms.k.Contracts.Get(ctx, msg.ContractName)

	if err != nil {
		if errors.Is(err, collections.ErrNotFound) {
			// Generate a default contract
			contract = zktx.Contract{
				Verifier:    "risczero",
				ProgramId:   "390d14c0c0a3f5eaede8e9b43db2a3b911780cebe46b70ca8fd745d3ca60691d",
				StateDigest: []byte{0},
			}
		} else {
			return nil, err
		}
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

		// TODO don't harcode this
		// TODO: don't know why, but last byte is a \n
		verifierCmd := exec.Command("/home/maximilien/risczerotuto-helloworld/hello-world/target/debug/host", "verify", contract.ProgramId, "proof.json", string(msg.InitialState[0]), string(msg.FinalState[0]))
		grepOut, _ := verifierCmd.StderrPipe()
		verifierCmd.Start()
		err = verifierCmd.Wait()

		if err != nil {
			grepBytes, _ := io.ReadAll(grepOut)
			fmt.Println(string(grepBytes))
			return nil, fmt.Errorf("verifier failed. Exit code: %s", err)
		}

	} else if contract.Verifier == "groth16-twistededwards-BN254" {
		var proof Groth16Proof
		err = json.Unmarshal(msg.Proof, &proof)
		if err != nil {
			return nil, fmt.Errorf("failed to unmarshal groth16 proof: %s", err)
		}

		// make an io reader from bytes
		proofReader := bytes.NewReader(proof.Proof)
		g16p := groth16.NewProof(ecc.BN254)
		_, err = g16p.ReadFrom(proofReader)
		if err != nil {
			return nil, fmt.Errorf("failed to parse groth16 proof: %s", err)
		}

		proofReader = bytes.NewReader(proof.VerifyingKey)
		vk := groth16.NewVerifyingKey(ecc.BN254)
		_, err := vk.ReadFrom(proofReader)
		if err != nil {
			return nil, fmt.Errorf("failed to parse groth16 vk: %s", err)
		}

		proofReader = bytes.NewReader(proof.PublicWitness)
		fid, _ := twistededwards.GetSnarkField(tedwards.BN254)
		witness, err := witness.New(fid)
		if err != nil {
			return nil, fmt.Errorf("failed to parse groth16 witness: %s", err)
		}
		_, err = witness.ReadFrom(proofReader)
		if err != nil {
			return nil, fmt.Errorf("failed to parse groth16 proof: %s", err)
		}
		publicWitness, err := witness.Public()
		if err != nil {
			return nil, fmt.Errorf("failed to parse groth16 proof: %s", err)
		}

		err = groth16.Verify(g16p, vk, publicWitness)

		if err != nil {
			return nil, fmt.Errorf("verifier failed: %s", err)
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

func (ms msgServer) RegisterContract(ctx context.Context, msg *zktx.MsgRegisterContract) (*zktx.MsgRegisterContractResponse, error) {
	if _, err := ms.k.Contracts.Get(ctx, msg.ContractName); err == nil || errors.Is(err, collections.ErrEncoding) {
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
