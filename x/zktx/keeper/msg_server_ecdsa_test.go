package keeper_test

import (
	"bytes"
	"crypto/rand"
	"crypto/sha256"
	"encoding/json"
	"fmt"
	"math/big"
	"testing"

	"github.com/hyle/hyle/zktx"
	"github.com/hyle/hyle/zktx/keeper"
	"github.com/stretchr/testify/require"

	"github.com/consensys/gnark/backend/groth16"
	"github.com/consensys/gnark/frontend"
	"github.com/consensys/gnark/frontend/cs/r1cs"

	"github.com/consensys/gnark-crypto/ecc"
	"github.com/consensys/gnark-crypto/ecc/secp256k1/ecdsa"
	"github.com/consensys/gnark/std/algebra/emulated/sw_emulated"
	"github.com/consensys/gnark/std/math/emulated"

	circuitecdsa "github.com/consensys/gnark/std/signature/ecdsa"
)

// GNARK circuit for ECDSA verification, this is implemented in emulated arithmetic so it's inefficient.
type ecdsaCircuit[T, S emulated.FieldParams] struct {
	Sig circuitecdsa.Signature[S]
	Msg emulated.Element[S]          `gnark:",public"`
	Pub circuitecdsa.PublicKey[T, S] `gnark:",public"`
}

func (c *ecdsaCircuit[T, S]) Define(api frontend.API) error {
	c.Pub.Verify(api, sw_emulated.GetCurveParams[T](), &c.Msg, &c.Sig)
	return nil
}

func main() (keeper.Groth16Proof, error) {
	// generate parameters
	privKey, _ := ecdsa.GenerateKey(rand.Reader)
	publicKey := privKey.PublicKey

	// sign
	msg := []byte("testing ECDSA (sha256)")
	md := sha256.New()
	sigBin, _ := privKey.Sign(msg, md)

	// check that the signature is correct
	flag, _ := publicKey.Verify(sigBin, msg, md)
	if !flag {
		return keeper.Groth16Proof{}, fmt.Errorf("invalid signature")
	}

	// unmarshal signature
	var sig ecdsa.Signature
	sig.SetBytes(sigBin)
	r, s := new(big.Int), new(big.Int)
	r.SetBytes(sig.R[:32])
	s.SetBytes(sig.S[:32])

	// compute the hash of the message as an integer
	dataToHash := make([]byte, len(msg))
	copy(dataToHash[:], msg[:])
	md.Reset()
	md.Write(dataToHash[:])
	hramBin := md.Sum(nil)
	hash := ecdsa.HashToInt(hramBin)

	circuit := ecdsaCircuit[emulated.Secp256k1Fp, emulated.Secp256k1Fr]{}
	witness_circuit := ecdsaCircuit[emulated.Secp256k1Fp, emulated.Secp256k1Fr]{
		Sig: circuitecdsa.Signature[emulated.Secp256k1Fr]{
			R: emulated.ValueOf[emulated.Secp256k1Fr](r),
			S: emulated.ValueOf[emulated.Secp256k1Fr](s),
		},
		Msg: emulated.ValueOf[emulated.Secp256k1Fr](hash),
		Pub: circuitecdsa.PublicKey[emulated.Secp256k1Fp, emulated.Secp256k1Fr]{
			X: emulated.ValueOf[emulated.Secp256k1Fp](privKey.PublicKey.A.X),
			Y: emulated.ValueOf[emulated.Secp256k1Fp](privKey.PublicKey.A.Y),
		},
	}

	r1cs, err := frontend.Compile(ecc.BN254.ScalarField(), r1cs.NewBuilder, &circuit)
	if err != nil {
		return keeper.Groth16Proof{}, err
	}

	// generating pk, vk
	pk, vk, err := groth16.Setup(r1cs)
	if err != nil {
		return keeper.Groth16Proof{}, err
	}

	witness, err := frontend.NewWitness(&witness_circuit, ecc.BN254.ScalarField())
	if err != nil {
		return keeper.Groth16Proof{}, err
	}
	publicWitness, err := witness.Public()
	if err != nil {
		return keeper.Groth16Proof{}, err
	}

	// generate the proof
	proof, err := groth16.Prove(r1cs, pk, witness)
	if err != nil {
		return keeper.Groth16Proof{}, err
	}

	// verify the proof
	err = groth16.Verify(proof, vk, publicWitness)
	if err != nil {
		return keeper.Groth16Proof{}, err
	}

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

func TestExecuteStateChangeGroth16ECDSA(t *testing.T) {
	f := initFixture(t)
	require := require.New(t)

	// Register the contract (TODO)
	contract := zktx.Contract{
		Verifier:    "gnark-groth16-te-BN254",
		StateDigest: []byte("initial_state"),
		ProgramId:   "program_id",
	}

	// set the contract state
	err := f.k.Contracts.Set(f.ctx, f.addrs[0].String(), contract)
	require.NoError(err)

	// create an array of bytes
	proof, _ := main()
	jsonproof, _ := json.Marshal(proof)

	// create a message
	msg := &zktx.MsgExecuteStateChange{
		ContractName: f.addrs[0].String(),
		Proof:        jsonproof,
		InitialState: []byte("initial_state"),
		FinalState:   []byte("final_state"),
	}

	// execute the message, this fails because VK is bad
	_, err = f.msgServer.ExecuteStateChange(f.ctx, msg)
	require.Error(err)

	// Fix VK
	contract.ProgramId = string(proof.VerifyingKey)
	err = f.k.Contracts.Set(f.ctx, f.addrs[0].String(), contract)
	require.NoError(err)

	// TODO: fix this, we need the message to match the expected structure.
	// // execute the message, this fails because VK is bad
	// _, err = f.msgServer.ExecuteStateChange(f.ctx, msg)
	// require.NoError(err)

	// st, _ := f.k.Contracts.Get(f.ctx, f.addrs[0].String())
	// require.Equal(st.StateDigest, []byte("final_state"))
}
