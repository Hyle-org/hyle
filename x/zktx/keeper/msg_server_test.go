package keeper_test

import (
	"bytes"
	"encoding/binary"
	"encoding/json"
	"fmt"
	"testing"

	"github.com/hyle/hyle/zktx"
	"github.com/hyle/hyle/zktx/keeper"
	"github.com/stretchr/testify/require"

	"github.com/consensys/gnark/backend/groth16"
	"github.com/consensys/gnark/frontend"
	"github.com/consensys/gnark/frontend/cs/r1cs"

	"github.com/consensys/gnark-crypto/ecc"
)

func TestUpdateParams(t *testing.T) {
	f := initFixture(t)
	require := require.New(t)

	testCases := []struct {
		name         string
		request      *zktx.MsgUpdateParams
		expectErrMsg string
	}{
		{
			name: "set invalid authority (not an address)",
			request: &zktx.MsgUpdateParams{
				Authority: "foo",
			},
			expectErrMsg: "invalid authority address",
		},
		{
			name: "set invalid authority (not defined authority)",
			request: &zktx.MsgUpdateParams{
				Authority: f.addrs[1].String(),
			},
			expectErrMsg: fmt.Sprintf("unauthorized, authority does not match the module's authority: got %s, want %s", f.addrs[1].String(), f.k.GetAuthority()),
		},
		{
			name: "set valid params",
			request: &zktx.MsgUpdateParams{
				Authority: f.k.GetAuthority(),
				Params:    zktx.Params{},
			},
			expectErrMsg: "",
		},
	}

	for _, tc := range testCases {
		tc := tc
		t.Run(tc.name, func(t *testing.T) {
			_, err := f.msgServer.UpdateParams(f.ctx, tc.request)
			if tc.expectErrMsg != "" {
				require.Error(err)
				require.ErrorContains(err, tc.expectErrMsg)
			} else {
				require.NoError(err)
			}
		})
	}
}

// GNARK circuit for ECDSA verification, this is implemented in emulated arithmetic so it's inefficient.
type randomCircuit struct {
	OtherData     frontend.Variable
	Input         frontend.Variable `gnark:",public"`
	Output        frontend.Variable `gnark:",public"`
	StillMoreData frontend.Variable
}

func (c *randomCircuit) Define(api frontend.API) error {
	c.StillMoreData = api.Add(c.Input, c.OtherData)
	api.AssertIsEqual(c.Output, c.StillMoreData)
	return nil
}

func generate_proof(a int, b int) (keeper.Groth16Proof, error) {
	circuit := randomCircuit{}

	// This bit would be done beforehand in a real circuit
	r1cs, err := frontend.Compile(ecc.BN254.ScalarField(), r1cs.NewBuilder, &circuit)
	if err != nil {
		return keeper.Groth16Proof{}, err
	}
	pk, vk, err := groth16.Setup(r1cs)
	if err != nil {
		return keeper.Groth16Proof{}, err
	}
	c := a + b
	circuit = randomCircuit{
		OtherData:     a,
		Input:         b,
		Output:        c,
		StillMoreData: 0,
	}
	witness, err := frontend.NewWitness(&circuit, ecc.BN254.ScalarField())
	if err != nil {
		return keeper.Groth16Proof{}, err
	}
	proof, err := groth16.Prove(r1cs, pk, witness)
	if err != nil {
		return keeper.Groth16Proof{}, err
	}

	// For testing convenience, verify the proof here
	publicWitness, err := witness.Public()
	if err != nil {
		return keeper.Groth16Proof{}, err
	}
	if err = groth16.Verify(proof, vk, publicWitness); err != nil {
		return keeper.Groth16Proof{}, err
	}

	// Simulates what the sender would have to do
	var proofBuf bytes.Buffer
	proof.WriteTo(&proofBuf)
	var vkBuf bytes.Buffer
	vk.WriteTo(&vkBuf)
	var publicWitnessBuf bytes.Buffer
	publicWitness.WriteTo(&publicWitnessBuf)
	return keeper.Groth16Proof{
		Proof:         proofBuf.Bytes(),
		VerifyingKey:  vkBuf.Bytes(),
		PublicWitness: publicWitnessBuf.Bytes(),
	}, nil
}

func TestExecuteStateChangeGroth16(t *testing.T) {
	f := initFixture(t)
	require := require.New(t)

	initial_buf := new(bytes.Buffer)
	var num uint16 = 1
	if err := binary.Write(initial_buf, binary.BigEndian, num); err != nil {
		t.Fatal(err)
	}
	output_buf := new(bytes.Buffer)
	num = 4
	if err := binary.Write(output_buf, binary.BigEndian, num); err != nil {
		t.Fatal(err)
	}

	// Register the contract (TODO)
	contract := zktx.Contract{
		Verifier:    "gnark-groth16-te-BN254",
		StateDigest: initial_buf.Bytes(),
		ProgramId:   "program_id",
	}

	// Set the initial state
	err := f.k.Contracts.Set(f.ctx, f.addrs[0].String(), contract)
	require.NoError(err)

	// Generate the proof and marshal it
	proof, err := generate_proof(3, 1)
	if err != nil {
		t.Fatal(err)
	}
	jsonproof, _ := json.Marshal(proof)

	// Create the massage, passing the jsoned proof
	msg := &zktx.MsgExecuteStateChange{
		ContractName: f.addrs[0].String(),
		Proof:        jsonproof,
		InitialState: initial_buf.Bytes(),
		FinalState:   output_buf.Bytes(),
	}

	// execute the message, this fails because VK is bad
	_, err = f.msgServer.ExecuteStateChange(f.ctx, msg)
	require.Error(err)

	// Fix VK
	contract.ProgramId = string(proof.VerifyingKey)
	err = f.k.Contracts.Set(f.ctx, f.addrs[0].String(), contract)
	require.NoError(err)

	// execute the message, this time succeeding
	_, err = f.msgServer.ExecuteStateChange(f.ctx, msg)
	require.NoError(err)

	st, _ := f.k.Contracts.Get(f.ctx, f.addrs[0].String())
	require.Equal(st.StateDigest, output_buf.Bytes())
}
