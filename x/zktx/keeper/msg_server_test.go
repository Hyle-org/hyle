package keeper_test

import (
	"bytes"
	"encoding/json"
	"testing"

	"github.com/hyle/hyle/zktx"
	"github.com/hyle/hyle/zktx/keeper/gnark"
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
			Input:     []frontend.Variable{initial_state},
			Output:    []frontend.Variable{end_state},
			Sender:    uints.NewU8Array([]byte("toto")), // We expect only the sender as this is the "auth contract"
			Caller:    uints.NewU8Array([]byte("")),
			BlockTime: 0,
			BlockNb:   0,
			TxHash:    uints.NewU8Array([]byte("TODO")),
		},
		StillMoreData: 0,
	}

	proof, err := generate_proof(&circuit)
	if err != nil {
		t.Fatal(err)
	}
	jsonproof, _ := json.Marshal(proof)

	// See below for details
	initial_state_witness := proof.PublicWitness[12+32 : 12+32+32*1]
	final_state_witness := proof.PublicWitness[12+32+32*1 : 12+32+32*1+1*32]

	// Setup contract
	_, err = f.msgServer.RegisterContract(f.ctx, &zktx.MsgRegisterContract{
		Owner:        f.addrs[0].String(),
		Verifier:     "gnark-groth16-te-BN254",
		ProgramId:    "bad_program_id",
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
				InitialState: []byte("bad_initial_state"),
				FinalState:   []byte("bad_final_state bad_final_states"), // This is padded so we get the error we want
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
	require.ErrorContains(err, "invalid initial state")

	msg.StateChanges[0].InitialState = initial_state_witness
	_, err = f.msgServer.ExecuteStateChange(f.ctx, msg)
	require.ErrorContains(err, "failed to unmarshal proof")

	msg.StateChanges[0].Proof = jsonproof
	_, err = f.msgServer.ExecuteStateChange(f.ctx, msg)
	require.ErrorContains(err, "verifying key does not match the known VK")

	// Fix VK (TODO: do this via a message)
	contract, err := f.k.Contracts.Get(f.ctx, contract_name)
	require.NoError(err)
	contract.ProgramId = string(proof.VerifyingKey)
	err = f.k.Contracts.Set(f.ctx, contract_name, contract)
	require.NoError(err)

	_, err = f.msgServer.ExecuteStateChange(f.ctx, msg)
	require.ErrorContains(err, "incorrect final state")

	msg.StateChanges[0].FinalState = final_state_witness

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
			Input:     inp[0:2],
			Output:    inp[2:4],
			Sender:    uints.NewU8Array([]byte("toto")), // We expect only the sender as this is the "auth contract""
			Caller:    uints.NewU8Array([]byte("")),
			BlockTime: 0,
			BlockNb:   0,
			TxHash:    uints.NewU8Array([]byte("TODO")),
		},
	}

	proof, err := generate_proof(&circuit)
	if err != nil {
		t.Fatal(err)
	}
	jsonproof, _ := json.Marshal(proof)

	// We pass serialized data and reconstruct it out-of-band as that happens to be the easiest solution ATM.
	// This is overall not great.
	// We need to skip 3 u32: the # of public items, the # of private items, and then the number of public items again (vector serialization in go)
	// Then we skip the version felt, and then we're good to go.
	// For this curve it's 32 bytes per felt.
	initial_state_witness := proof.PublicWitness[12+32 : 12+32+32*2]
	final_state_witness := proof.PublicWitness[12+32+32*2 : 12+32+32*2+2*32]

	_, err = f.msgServer.RegisterContract(f.ctx, &zktx.MsgRegisterContract{
		Owner:        f.addrs[0].String(),
		Verifier:     "gnark-groth16-te-BN254",
		ProgramId:    string(proof.VerifyingKey),
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
				InitialState: initial_state_witness,
				FinalState:   final_state_witness,
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

func TestExecuteSampleAttackPayload(t *testing.T) {
	f := initFixture(t)
	require := require.New(t)

	contract_name := "test-contract"
	sender := "toto.test-contract"

	// Generate the proof and marshal it
	circuit := statefulCircuit{
		HyleCircuit: gnark.HyleCircuit{
			Version:   1,
			Input:     []frontend.Variable{1},
			Output:    []frontend.Variable{4},
			Sender:    uints.NewU8Array([]byte("toto")), // We expect only the sender as this is the "auth contract""
			Caller:    uints.NewU8Array([]byte("")),
			BlockTime: 0,
			BlockNb:   0,
			TxHash:    uints.NewU8Array([]byte("TODO")),
		},
		OtherData:     3,
		StillMoreData: 0,
	}

	proof, err := generate_proof(&circuit)
	if err != nil {
		t.Fatal(err)
	}
	valid_initial_state_witness := proof.PublicWitness[12+32 : 12+32+32]
	final_state_witness := proof.PublicWitness[12+32+32 : 12+32+32+32]

	// Attack: we actually generate a proof from a different initial state
	circuit = statefulCircuit{
		HyleCircuit: gnark.HyleCircuit{
			Version:   1,
			Input:     []frontend.Variable{4},
			Output:    []frontend.Variable{4},
			Sender:    uints.NewU8Array([]byte("toto")), // We expect only the sender as this is the "auth contract""
			Caller:    uints.NewU8Array([]byte("")),
			BlockTime: 0,
			BlockNb:   0,
			TxHash:    uints.NewU8Array([]byte("TODO")),
		},
		OtherData:     0,
		StillMoreData: 0,
	}

	proof, err = generate_proof(&circuit)
	if err != nil {
		t.Fatal(err)
	}
	jsonproof, _ := json.Marshal(proof)

	_, err = f.msgServer.RegisterContract(f.ctx, &zktx.MsgRegisterContract{
		Owner:        f.addrs[0].String(),
		Verifier:     "gnark-groth16-te-BN254",
		ProgramId:    string(proof.VerifyingKey),
		StateDigest:  valid_initial_state_witness,
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
				InitialState: valid_initial_state_witness, // attack: we pretend the initial state is the valid one but our proof is for the attacked one
				FinalState:   final_state_witness,
			},
		},
	}

	// Execute the message and we detect the attack
	_, err = f.msgServer.ExecuteStateChange(f.ctx, msg)
	require.ErrorContains(err, "incorrect initial state")

	// This would however totally fly as a pure verification message
	verifyMsg := &zktx.MsgVerifyProof{
		ContractName: contract_name,
		Proof:        jsonproof,
	}
	_, err = f.msgServer.VerifyProof(f.ctx, verifyMsg)
	require.NoError(err)
}
