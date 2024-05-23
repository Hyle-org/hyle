package keeper_test

import (
	"bytes"
	"encoding/json"
	"testing"

	"github.com/hyle-org/hyle/x/zktx"
	"github.com/hyle-org/hyle/x/zktx/keeper/gnark"
	"github.com/stretchr/testify/require"

	"github.com/consensys/gnark/backend/groth16"
	"github.com/consensys/gnark/frontend"
	"github.com/consensys/gnark/frontend/cs/r1cs"
	"github.com/consensys/gnark/std/math/uints"

	"github.com/consensys/gnark-crypto/ecc"
)

// Sample GNARK circuit for stateful transactions, with redundant private variables
type statefulCircuit struct {
	gnark.HyleCircuit
	OtherData     frontend.Variable
	StillMoreData frontend.Variable
}

type longStatefulCircuit struct {
	gnark.HyleCircuit
}

func (c *statefulCircuit) Define(api frontend.API) error {
	c.StillMoreData = api.Add(c.Input[0], c.OtherData)
	api.AssertIsEqual(c.Output[0], c.StillMoreData)
	return nil
}

func (c *longStatefulCircuit) Define(api frontend.API) error {
	temp := api.Add(c.Input[0], c.Input[1])
	api.AssertIsEqual(temp, c.Output[1])
	return nil
}

func generate_proof[C frontend.Circuit](circuit C) (gnark.Groth16Proof, error) {
	// Prep the witness first as compilation modifies the circuit.
	witness, err := frontend.NewWitness(circuit, ecc.BN254.ScalarField())
	if err != nil {
		return gnark.Groth16Proof{}, err
	}

	// This bit would be done beforehand in a real circuit
	r1cs, err := frontend.Compile(ecc.BN254.ScalarField(), r1cs.NewBuilder, circuit)
	if err != nil {
		return gnark.Groth16Proof{}, err
	}
	pk, vk, err := groth16.Setup(r1cs)
	if err != nil {
		return gnark.Groth16Proof{}, err
	}

	proof, err := groth16.Prove(r1cs, pk, witness)
	if err != nil {
		return gnark.Groth16Proof{}, err
	}

	// For testing convenience, verify the proof here
	publicWitness, err := witness.Public()
	if err != nil {
		return gnark.Groth16Proof{}, err
	}
	if err = groth16.Verify(proof, vk, publicWitness); err != nil {
		return gnark.Groth16Proof{}, err
	}

	// Simulates what the sender would have to do
	var proofBuf bytes.Buffer
	proof.WriteTo(&proofBuf)
	var vkBuf bytes.Buffer
	vk.WriteTo(&vkBuf)
	var publicWitnessBuf bytes.Buffer
	publicWitness.WriteTo(&publicWitnessBuf)
	return gnark.Groth16Proof{
		Proof:         proofBuf.Bytes(),
		VerifyingKey:  vkBuf.Bytes(),
		PublicWitness: publicWitnessBuf.Bytes(),
	}, nil
}

func TestExecuteStateChangeGroth16(t *testing.T) {
	f := initFixture(t)
	require := require.New(t)

	// Parameters:
	var initial_state = 1
	var end_state = 4
	contract_name := "test-contract"
	sender := "toto.test-contract"

	// Generate the proof and marshal it
	circuit := statefulCircuit{
		OtherData: 3,
		HyleCircuit: gnark.HyleCircuit{
			Version:   1,
			InputLen:  1,
			Input:     []frontend.Variable{initial_state},
			OutputLen: 1,
			Output:    []frontend.Variable{end_state},
			SenderLen: len("toto." + contract_name),
			Sender:    uints.NewU8Array([]byte("toto." + contract_name)),
			CallerLen: 0,
			Caller:    uints.NewU8Array([]byte("")),
			BlockTime: 0,
			BlockNb:   0,
			TxHashLen: len("TODO"),
			TxHash:    uints.NewU8Array([]byte("TODO")),
		},
		StillMoreData: 0,
	}

	proof, err := generate_proof(&circuit)
	if err != nil {
		t.Fatal(err)
	}
	jsonproof, _ := json.Marshal(proof)

	initial_state_witness := []byte{byte(initial_state)}
	final_state_witness := []byte{byte(end_state)}

	// Setup contract
	_, err = f.msgServer.RegisterContract(f.ctx, &zktx.MsgRegisterContract{
		Owner:        f.addrs[0].String(),
		Verifier:     "gnark-groth16-te-BN254",
		ProgramId:    []byte("bad_program_id"),
		StateDigest:  initial_state_witness,
		ContractName: contract_name,
	})
	require.NoError(err)

	// Create a broken message.
	msg := &zktx.MsgExecuteStateChange{
		HyleSender: "noone.bad_contract",
		BlockTime:  0,
		BlockNb:    0,
		TxHash:     []byte("TODO"),
		StateChanges: []*zktx.StateChange{
			{
				ContractName: "bad_contract",
				Proof:        []byte("bad_proof"),
			},
		},
	}

	_, err = f.msgServer.ExecuteStateChange(f.ctx, msg)
	require.ErrorContains(err, "no state is registered")

	msg.HyleSender = sender
	_, err = f.msgServer.ExecuteStateChange(f.ctx, msg)
	require.ErrorContains(err, "invalid sender contract")

	msg.StateChanges[0].ContractName = contract_name
	_, err = f.msgServer.ExecuteStateChange(f.ctx, msg)
	require.ErrorContains(err, "failed to unmarshal proof")

	msg.StateChanges[0].Proof = jsonproof
	_, err = f.msgServer.ExecuteStateChange(f.ctx, msg)
	require.ErrorContains(err, "verifying key does not match the known VK")

	// Fix VK (TODO: do this via a message)
	contract, err := f.k.Contracts.Get(f.ctx, contract_name)
	require.NoError(err)
	contract.ProgramId = proof.VerifyingKey
	err = f.k.Contracts.Set(f.ctx, contract_name, contract)
	require.NoError(err)

	// execute the message, this time succeeding
	_, err = f.msgServer.ExecuteStateChange(f.ctx, msg)
	require.NoError(err)

	// Check output state is correct
	st, _ := f.k.Contracts.Get(f.ctx, contract_name)
	require.Equal(st.StateDigest, final_state_witness)
}

func TestExecuteLongStateChangeGroth16(t *testing.T) {
	f := initFixture(t)
	require := require.New(t)

	// Parameters:
	var initial_state = []int{1, 3}
	var end_state = []int{234, 4}
	contract_name := "test-contract"
	sender := "toto.test-contract"

	inp := [4]frontend.Variable{initial_state[0], initial_state[1], end_state[0], end_state[1]}
	// Generate the proof and marshal it
	circuit := longStatefulCircuit{
		HyleCircuit: gnark.HyleCircuit{
			Version:   1,
			InputLen:  2,
			Input:     inp[0:2],
			OutputLen: 2,
			Output:    inp[2:4],
			SenderLen: len("toto." + contract_name),
			Sender:    uints.NewU8Array([]byte("toto." + contract_name)),
			CallerLen: 0,
			Caller:    uints.NewU8Array([]byte("")),
			BlockTime: 0,
			BlockNb:   0,
			TxHashLen: len("TODO"),
			TxHash:    uints.NewU8Array([]byte("TODO")),
		},
	}

	proof, err := generate_proof(&circuit)
	if err != nil {
		t.Fatal(err)
	}
	jsonproof, _ := json.Marshal(proof)

	_, _, witness, err := proof.ParseProof()
	require.NoError(err)
	data, err := proof.ExtractData(witness)
	require.NoError(err)

	initial_state_witness := data.InitialState
	final_state_witness := data.NextState
	require.Equal(initial_state_witness, []byte{byte(initial_state[0]), byte(initial_state[1])})
	require.Equal(final_state_witness, []byte{byte(end_state[0]), byte(end_state[1])})

	require.Equal(data.Sender, "toto."+contract_name)
	require.Equal(data.Caller, "")
	require.Equal(data.BlockTime, uint64(0))
	require.Equal(data.BlockNumber, uint64(0))
	require.Equal(data.TxHash, []byte("TODO"))

	_, err = f.msgServer.RegisterContract(f.ctx, &zktx.MsgRegisterContract{
		Owner:        f.addrs[0].String(),
		Verifier:     "gnark-groth16-te-BN254",
		ProgramId:    proof.VerifyingKey,
		StateDigest:  initial_state_witness,
		ContractName: contract_name,
	})
	require.NoError(err)

	// Create the message
	msg := &zktx.MsgExecuteStateChange{
		HyleSender: sender,
		BlockTime:  0,
		BlockNb:    0,
		TxHash:     []byte("TODO"),
		StateChanges: []*zktx.StateChange{
			{
				ContractName: contract_name,
				Proof:        jsonproof,
			},
		},
	}

	// execute the message, this time succeeding
	_, err = f.msgServer.ExecuteStateChange(f.ctx, msg)
	require.NoError(err)

	// Check output state is correct
	st, _ := f.k.Contracts.Get(f.ctx, contract_name)
	require.Equal(st.StateDigest, final_state_witness)
}

func TestUnmarshallHyleOutput(t *testing.T) {
	require := require.New(t)
	raw_json := "{\"version\":1,\"initial_state\":[0,0,0,1],\"next_state\":[0,0,0,15],\"sender\":\"\",\"caller\":\"\",\"block_number\":0,\"block_time\":0,\"tx_hash\":[1],\"program_outputs\":null}"
	var output zktx.HyleOutput
	err := json.Unmarshal([]byte(raw_json), &output)
	require.NoError(err)
}
