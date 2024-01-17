package keeper

import (
	"bytes"
	"context"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"strings"

	"cosmossdk.io/collections"
	"github.com/hyle/hyle/zktx"
)

type msgServer struct {
	k Keeper
}

var _ zktx.MsgServer = msgServer{}

// NewMsgServerImpl returns an implementation of the module MsgServer interface.
func NewMsgServerImpl(keeper Keeper) zktx.MsgServer {
	return &msgServer{k: keeper}
}

func (ms msgServer) ExecuteStateChange(ctx context.Context, msg *zktx.MsgExecuteStateChange) (*zktx.MsgExecuteStateChangeResponse, error) {
	// Print some stuff
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

	// TODO: initialise contract at certain addresses
	//if msg.ContractAddress != "mini1s35tpv67eafejyvpxxdtn4e7dgm8whmm07a2x6" {
	//	return nil, fmt.Errorf("Unknown contract address: %s", msg.ContractAddress)
	//}

	// Hardcoded for now
	if state.Verifier != "risczero" {
		return nil, fmt.Errorf("invalid verifier: expected %s, got %s", "risczero", state.Verifier)
	}

	if !bytes.Equal(state.StateDigest, msg.InitialState) {
		return nil, fmt.Errorf("invalid initial state, expected %x, got %x", state.StateDigest, msg.InitialState)
	}

	// Save proof to a local file
	err = os.WriteFile("proof.json", msg.Proof, 0644)

	if err != nil {
		return nil, fmt.Errorf("failed to write proof to file: %s", err)
	}

	verifierCmd := exec.Command("/Volumes/Samsung_T5/Programming/hylé/risczero/target/debug/host", "verify", "5489d205ec0d546ed93b75d890d4278477ae7d2f3fb180b3a853fcece6719814", "proof.json", hex.EncodeToString(msg.InitialState), hex.EncodeToString(msg.FinalState))
	grepOut, _ := verifierCmd.StderrPipe()
	verifierCmd.Start()
	err = verifierCmd.Wait()

	if err != nil {
		grepBytes, _ := io.ReadAll(grepOut)
		fmt.Println(string(grepBytes))
		return nil, fmt.Errorf("verifier failed. Exit code: %s", err)
	}

	// Update state
	state.StateDigest = msg.FinalState

	if err := ms.k.ContractStates.Set(ctx, msg.ContractAddress, state); err != nil {
		return nil, err
	}

	return &zktx.MsgExecuteStateChangeResponse{}, nil
}

///// Default stuff

// IncrementCounter defines the handler for the MsgIncrementCounter message.
func (ms msgServer) IncrementCounter(ctx context.Context, msg *zktx.MsgIncrementCounter) (*zktx.MsgIncrementCounterResponse, error) {
	if _, err := ms.k.addressCodec.StringToBytes(msg.Sender); err != nil {
		return nil, fmt.Errorf("invalid sender address: %w", err)
	}

	counter, err := ms.k.Counter.Get(ctx, msg.Sender)
	if err != nil && !errors.Is(err, collections.ErrNotFound) {
		return nil, err
	}

	counter++

	if err := ms.k.Counter.Set(ctx, msg.Sender, counter); err != nil {
		return nil, err
	}

	return &zktx.MsgIncrementCounterResponse{}, nil
}

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
