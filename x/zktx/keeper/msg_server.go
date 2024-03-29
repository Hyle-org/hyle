package keeper

import (
	"bytes"
	"context"
	"encoding/hex"
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
	state, err := ms.k.ContractStates.Get(ctx, msg.ContractAddress)

	if err != nil {
		if errors.Is(err, collections.ErrNotFound) {
			// Generate a default state
			state = zktx.ContractState{
				Verifier:    "risczero",
				ProgramId:   "5489d205ec0d546ed93b75d890d4278477ae7d2f3fb180b3a853fcece6719814",
				StateDigest: []byte{0},
			}
		} else {
			return nil, err
		}
	}

	if !bytes.Equal(state.StateDigest, msg.InitialState) {
		return nil, fmt.Errorf("invalid initial state, expected %x, got %x", state.StateDigest, msg.InitialState)
	}

	if state.Verifier == "risczero" {
		// Save proof to a local file
		err = os.WriteFile("proof.json", msg.Proof, 0644)

		if err != nil {
			return nil, fmt.Errorf("failed to write proof to file: %s", err)
		}

		// TODO don't harcode this
		verifierCmd := exec.Command("/Volumes/Samsung_T5/Programming/hylé/risczero/target/debug/host", "verify", "5489d205ec0d546ed93b75d890d4278477ae7d2f3fb180b3a853fcece6719814", "proof.json", hex.EncodeToString(msg.InitialState), hex.EncodeToString(msg.FinalState))
		grepOut, _ := verifierCmd.StderrPipe()
		verifierCmd.Start()
		err = verifierCmd.Wait()

		if err != nil {
			grepBytes, _ := io.ReadAll(grepOut)
			fmt.Println(string(grepBytes))
			return nil, fmt.Errorf("verifier failed. Exit code: %s", err)
		}
	} else if state.Verifier == "groth16-twistededwards-BN254" {
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
			return nil, fmt.Errorf("Verifier failed: %s", err)
		}
	} else {
		return nil, fmt.Errorf("unknown verifier %s", state.Verifier)
	}

	// Update state
	state.StateDigest = msg.FinalState

	if err := ms.k.ContractStates.Set(ctx, msg.ContractAddress, state); err != nil {
		return nil, err
	}

	return &zktx.MsgExecuteStateChangeResponse{}, nil
}

func (ms msgServer) RegisterContract(ctx context.Context, msg *zktx.MsgRegisterContract) (*zktx.MsgRegisterContractResponse, error) {
	return nil, fmt.Errorf("not implemented")
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
