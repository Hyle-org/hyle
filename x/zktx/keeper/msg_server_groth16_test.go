package keeper_test

import (
	"bytes"
	"crypto/sha256"
	"encoding/json"
	"fmt"
	"testing"

	"github.com/consensys/gnark-crypto/ecc"
	"github.com/consensys/gnark/backend/groth16"
	"github.com/consensys/gnark/frontend"
	"github.com/consensys/gnark/frontend/cs/r1cs"
	"github.com/consensys/gnark/std/hash/sha3"
	"github.com/consensys/gnark/std/math/uints"
	"github.com/hyle-org/hyle/x/zktx"
	"github.com/hyle-org/hyle/x/zktx/keeper/gnark"
	"github.com/stretchr/testify/require"
	gosha3 "golang.org/x/crypto/sha3"
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

func frontendVarToU8(v []frontend.Variable) []uints.U8 {
	t := make([]uints.U8, len(v))
	for i, b := range v {
		t[i].Val = b
	}
	return t
}

func computeCircuitPayloadHash(api frontend.API, c *gnark.HyleCircuit) error {
	uapi, err := uints.New[uints.U64](api)
	if err != nil {
		return err
	}
	newHasher, err := sha3.NewLegacyKeccak256(api)
	if err != nil {
		return err
	}
	input := frontendVarToU8(c.Input)
	newHasher.Write(input)
	payloadHash := newHasher.Sum()
	if a, b := len(payloadHash), len(c.PayloadHash); a != b {
		return fmt.Errorf("payload hash length mismatch %d != %d", a, b)
	}
	for i := 0; i < len(payloadHash); i++ {
		uapi.ByteAssertEq(payloadHash[i], c.PayloadHash[i])
	}
	return nil
}

func (c *statefulCircuit) Define(api frontend.API) error {
	c.StillMoreData = api.Add(c.Input[0], c.OtherData)
	api.AssertIsEqual(c.Output[0], c.StillMoreData)
	return computeCircuitPayloadHash(api, &c.HyleCircuit)
}

func (c *longStatefulCircuit) Define(api frontend.API) error {
	temp := api.Add(c.Input[0], c.Input[1])
	api.AssertIsEqual(temp, c.Output[1])
	return computeCircuitPayloadHash(api, &c.HyleCircuit)
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

	// Simulates what the identity would have to do
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

func computePayloadHash(payload []byte) ([]byte, error) {
	hasher := gosha3.NewLegacyKeccak256()
	if _, err := hasher.Write(payload); err != nil {
		return nil, err
	}
	return hasher.Sum(nil), nil
}

func TestExecuteStateChangesGroth16(t *testing.T) {
	f := initFixture(t)
	require := require.New(t)

	// Parameters:
	var initial_state = 1
	var end_state = 4
	contract_name := "test-contract"

	payloadHash, err := computePayloadHash([]byte{byte(initial_state)})
	require.NoError(err)

	// Generate the proof and marshal it
	circuit := statefulCircuit{
		OtherData: 3,
		HyleCircuit: gnark.HyleCircuit{
			Version:     1,
			InputLen:    1,
			Input:       []frontend.Variable{initial_state},
			OutputLen:   1,
			Output:      []frontend.Variable{end_state},
			IdentityLen: len("toto." + contract_name),
			Identity:    gnark.ToArray256([]byte("toto." + contract_name)),
			TxHash:      gnark.ToArray64([]byte("TODO")),
			PayloadHash: gnark.ToArray32(payloadHash),
			Success:     1,
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

	f.ctx = f.ctx.WithTxBytes([]byte("FakeTx"))
	h := sha256.New()
	h.Write(f.ctx.TxBytes())
	txHash := h.Sum(nil)

	// First - send the payload
	_, err = f.msgServer.PublishPayloads(f.ctx, &zktx.MsgPublishPayloads{
		Payloads: []*zktx.Payload{
			{
				ContractName: contract_name,
				Data:         initial_state_witness,
			},
		},
	})
	require.NoError(err)

	// Create a broken message.
	msg := &zktx.MsgPublishPayloadProof{
		TxHash:       txHash,
		PayloadIndex: 0,
		ContractName: "bad_contract",
		Proof:        []byte("bad_proof"),
	}

	_, err = f.msgServer.PublishPayloadProof(f.ctx, msg)
	require.ErrorContains(err, "invalid contract - no state is registered")

	msg.ContractName = contract_name
	_, err = f.msgServer.PublishPayloadProof(f.ctx, msg)
	require.ErrorContains(err, "failed to unmarshal proof")

	msg.Proof = jsonproof
	_, err = f.msgServer.PublishPayloadProof(f.ctx, msg)
	require.ErrorContains(err, "verifying key does not match the known VK")

	// Fix VK (TODO: do this via a message)
	contract, err := f.k.Contracts.Get(f.ctx, contract_name)
	require.NoError(err)
	contract.ProgramId = proof.VerifyingKey
	err = f.k.Contracts.Set(f.ctx, contract_name, contract)
	require.NoError(err)

	// execute the message, this time succeeding
	_, err = f.msgServer.PublishPayloadProof(f.ctx, msg)
	require.NoError(err)

	// Check output state is correct
	st, _ := f.k.Contracts.Get(f.ctx, contract_name)
	require.Equal(st.StateDigest, final_state_witness)
}

func TestBadPayloadGroth16(t *testing.T) {
	require := require.New(t)

	// Parameters:
	var initial_state = 1
	var end_state = 4
	contract_name := "test-contract"

	// Generate the proof and marshal it
	circuit := statefulCircuit{
		OtherData: 3,
		HyleCircuit: gnark.HyleCircuit{
			Version:     1,
			InputLen:    1,
			Input:       []frontend.Variable{initial_state},
			OutputLen:   1,
			Output:      []frontend.Variable{end_state},
			IdentityLen: len("toto." + contract_name),
			Identity:    gnark.ToArray256([]byte("toto." + contract_name)),
			TxHash:      gnark.ToArray64([]byte("TODO")),
			// do not set PayloadHash
			Success: 1,
		},
		StillMoreData: 0,
	}

	_, err := generate_proof(&circuit)
	require.Error(err, "payload hash should mismatch")
}

func TestExecuteLongStateChangeGroth16(t *testing.T) {
	f := initFixture(t)
	require := require.New(t)

	// Parameters:
	var initial_state = []int{1, 3}
	var end_state = []int{234, 4}
	contract_name := "test-contract"

	initial_state_bytes := make([]byte, len(initial_state))
	for i, state := range initial_state {
		initial_state_bytes[i] = byte(state)
	}
	payloadHash, err := computePayloadHash(initial_state_bytes)
	require.NoError(err)

	inp := [4]frontend.Variable{initial_state[0], initial_state[1], end_state[0], end_state[1]}
	// Generate the proof and marshal it
	circuit := longStatefulCircuit{
		HyleCircuit: gnark.HyleCircuit{
			Version:     1,
			InputLen:    2,
			Input:       inp[0:2],
			OutputLen:   2,
			Output:      inp[2:4],
			IdentityLen: len("toto." + contract_name),
			Identity:    gnark.ToArray256([]byte("toto." + contract_name)),
			TxHash:      gnark.ToArray64([]byte("TODO")),
			Success:     1,
			PayloadHash: gnark.ToArray32(payloadHash),
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

	require.Equal(data.Identity, "toto."+contract_name)
	require.Equal(data.TxHash, append([]byte("TODO"), make([]byte, 60)...))

	_, err = f.msgServer.RegisterContract(f.ctx, &zktx.MsgRegisterContract{
		Owner:        f.addrs[0].String(),
		Verifier:     "gnark-groth16-te-BN254",
		ProgramId:    proof.VerifyingKey,
		StateDigest:  initial_state_witness,
		ContractName: contract_name,
	})
	require.NoError(err)

	f.ctx = f.ctx.WithTxBytes([]byte("FakeTx"))
	h := sha256.New()
	h.Write(f.ctx.TxBytes())
	txHash := h.Sum(nil)

	// First - send the payload
	_, err = f.msgServer.PublishPayloads(f.ctx, &zktx.MsgPublishPayloads{
		Payloads: []*zktx.Payload{
			{
				ContractName: contract_name,
				Data:         initial_state_witness,
			},
		},
	})
	require.NoError(err)

	// Create the message
	msg := &zktx.MsgPublishPayloadProof{
		TxHash:       txHash,
		PayloadIndex: 0,
		ContractName: contract_name,
		Proof:        jsonproof,
	}

	// execute the message, this time succeeding
	_, err = f.msgServer.PublishPayloadProof(f.ctx, msg)
	require.NoError(err)

	// Check output state is correct
	st, _ := f.k.Contracts.Get(f.ctx, contract_name)
	require.Equal(st.StateDigest, final_state_witness)
}

/*
func TestBadOrigins(t *testing.T) {
	f := initFixture(t)
	require := require.New(t)

	// Generate the proof and marshal it
	circuit := statefulCircuit{
		OtherData: 0,
		HyleCircuit: gnark.HyleCircuit{
			Version:     1,
			InputLen:    1,
			Input:       []frontend.Variable{0},
			OutputLen:   1,
			Output:      []frontend.Variable{0},
			IdentityLen: len("toto.test"),
			Identity:    gnark.ToArray256([]byte("toto.test")),
			TxHash:      gnark.ToArray64([]byte("TODO")),
		},
		StillMoreData: 0,
	}

	proof, err := generate_proof(&circuit)
	if err != nil {
		t.Fatal(err)
	}
	toto_test_proof, _ := json.Marshal(proof)

	// Setup contract
	_, err = f.msgServer.RegisterContract(f.ctx, &zktx.MsgRegisterContract{
		Owner:        f.addrs[0].String(),
		Verifier:     "gnark-groth16-te-BN254",
		ProgramId:    proof.VerifyingKey,
		StateDigest:  []byte{byte(0)},
		ContractName: "test",
	})
	require.NoError(err)

	circuit = statefulCircuit{
		OtherData: 0,
		HyleCircuit: gnark.HyleCircuit{
			Version:     1,
			InputLen:    1,
			Input:       []frontend.Variable{0},
			OutputLen:   1,
			Output:      []frontend.Variable{0},
			IdentityLen: len("toto.jack_test"),
			Identity:    gnark.ToArray256([]byte("toto.jack_test")),
			TxHash:      gnark.ToArray64([]byte("TODO")),
		},
		StillMoreData: 0,
	}

	proof, err = generate_proof(&circuit)
	if err != nil {
		t.Fatal(err)
	}
	jack_test_proof, _ := json.Marshal(proof)

	// Setup contract
	_, err = f.msgServer.RegisterContract(f.ctx, &zktx.MsgRegisterContract{
		Owner:        f.addrs[0].String(),
		Verifier:     "gnark-groth16-te-BN254",
		ProgramId:    proof.VerifyingKey,
		StateDigest:  []byte{byte(0)},
		ContractName: "jack_test",
	})
	require.NoError(err)

	circuit = statefulCircuit{
		OtherData: 0,
		HyleCircuit: gnark.HyleCircuit{
			Version:     1,
			InputLen:    1,
			Input:       []frontend.Variable{0},
			OutputLen:   1,
			Output:      []frontend.Variable{0},
			IdentityLen: 0,
			Identity:    gnark.ToArray256([]byte("")),
			TxHash:      gnark.ToArray64([]byte("TODO")),
		},
		StillMoreData: 0,
	}

	proof, err = generate_proof(&circuit)
	if err != nil {
		t.Fatal(err)
	}
	anon_proof, _ := json.Marshal(proof)

	// Setup contract
	_, err = f.msgServer.RegisterContract(f.ctx, &zktx.MsgRegisterContract{
		Owner:        f.addrs[0].String(),
		Verifier:     "gnark-groth16-te-BN254",
		ProgramId:    proof.VerifyingKey,
		StateDigest:  []byte{byte(0)},
		ContractName: "anon",
	})
	require.NoError(err)

	// Create the message
	msg := &zktx.MsgPublishPayloadProof{
		TxHash:       []byte("FakeTx"),
		PayloadIndex: 0,
		PayloadHash:  []byte("bad_payload"),
		ContractName: contract_name,
		Proof:        jsonproof,
	}

	// First test: this works, the first sets the identity and the second of course works
	msg := &zktx.MsgExecuteStateChanges{
		StateChanges: []*zktx.StateChange{
			{
				ContractName: "test",
				Proof:        toto_test_proof,
			},
			{
				ContractName: "test",
				Proof:        toto_test_proof,
			},
		},
	}

	_, err = f.msgServer.ExecuteStateChanges(f.ctx, msg)
	require.NoError(err)

	// Fails: the identity is not the same
	msg = &zktx.MsgExecuteStateChanges{
		StateChanges: []*zktx.StateChange{
			{
				ContractName: "test",
				Proof:        toto_test_proof,
			},
			{
				ContractName: "jack_test",
				Proof:        jack_test_proof,
			},
		},
	}

	_, err = f.msgServer.ExecuteStateChanges(f.ctx, msg)
	require.ErrorContains(err, "verifier output does not match the expected identity")

	// Fails: the identity must be none
	msg = &zktx.MsgExecuteStateChanges{
		StateChanges: []*zktx.StateChange{
			{
				ContractName: "anon",
				Proof:        anon_proof,
			},
			{
				ContractName: "jack_test",
				Proof:        jack_test_proof,
			},
		},
	}

	_, err = f.msgServer.ExecuteStateChanges(f.ctx, msg)
	//require.ErrorContains(err, "verifier output does not match the expected identity")
	// TODO: this actually passes, we set the identity as the first we see. Maybe change this?
	require.NoError(err)

	// This succeeds: the anon contract does not expect any particular identity via ""
	msg = &zktx.MsgExecuteStateChanges{
		StateChanges: []*zktx.StateChange{
			{
				ContractName: "jack_test",
				Proof:        jack_test_proof,
			},
			{
				ContractName: "anon",
				Proof:        anon_proof,
			},
		},
	}

	_, err = f.msgServer.ExecuteStateChanges(f.ctx, msg)
	require.NoError(err)
}
*/
