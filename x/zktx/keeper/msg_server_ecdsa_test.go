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
	"github.com/consensys/gnark/test"

	"github.com/consensys/gnark-crypto/ecc"
	"github.com/consensys/gnark-crypto/ecc/secp256k1/ecdsa"
	"github.com/consensys/gnark/std/algebra/emulated/sw_emulated"
	"github.com/consensys/gnark/std/hash/sha3"
	"github.com/consensys/gnark/std/math/emulated"
	"github.com/consensys/gnark/std/math/uints"

	nativesha3 "github.com/ethereum/go-ethereum/crypto"

	circuitecdsa "github.com/consensys/gnark/std/signature/ecdsa"
)

// GNARK circuit for ECDSA verification, this is implemented in emulated arithmetic so it's inefficient.
type ecdsaCircuit[T, S emulated.FieldParams] struct {
	Version frontend.Variable   `gnark:",public"`
	Input   []frontend.Variable `gnark:",public"`
	Output  []frontend.Variable `gnark:",public"`
	Sender  []uints.U8          `gnark:",public"`
	Sig     circuitecdsa.Signature[S]
	Msg     emulated.Element[S] `gnark:",public"`
	Pub     circuitecdsa.PublicKey[T, S]
}

func (c *ecdsaCircuit[T, S]) Define(api frontend.API) error {
	// Verify address
	newHasher, err := sha3.NewLegacyKeccak256(api)
	if err != nil {
		return err
	}

	uapi, err := uints.New[uints.U64](api)
	if err != nil {
		return err
	}
	pubKeyBytes, err := pubKeyToBytes(api, &c.Pub)
	if err != nil {
		return err
	}
	newHasher.Write(pubKeyBytes)
	res := newHasher.Sum()

	for i := 0; i < 20; i++ {
		uapi.ByteAssertEq(c.Sender[i], res[i+12])
	}

	c.Pub.Verify(api, sw_emulated.GetCurveParams[T](), &c.Msg, &c.Sig)
	return nil
}

// The following two functions directly pulled from https://github.com/Consensys/gnark/discussions/802
func pubKeyToBytes[T, S emulated.FieldParams](api frontend.API, pubKey *circuitecdsa.PublicKey[T, S]) ([]uints.U8, error) {
	xLimbs := pubKey.X.Limbs
	yLimbs := pubKey.Y.Limbs

	u64api, err := uints.New[uints.U64](api)
	if err != nil {
		return nil, err
	}

	result := limbsToBytes(u64api, xLimbs)
	return append(result, limbsToBytes(u64api, yLimbs)...), nil
}

func limbsToBytes(u64api *uints.BinaryField[uints.U64], limbs []frontend.Variable) []uints.U8 {
	result := make([]uints.U8, 0, len(limbs)*8)
	for i := range limbs {
		u64 := u64api.ValueOf(limbs[len(limbs)-1-i])
		result = append(result, u64api.UnpackMSB(u64)...)
	}
	return result
}

func generate_ecdsa_proof(privKey *ecdsa.PrivateKey) (keeper.Groth16Proof, error) {
	publicKey := privKey.PublicKey
	pubkeyBytes := publicKey.A.RawBytes()
	ethAddress := nativesha3.Keccak256(pubkeyBytes[:])[12:]

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

	circuit := ecdsaCircuit[emulated.Secp256k1Fp, emulated.Secp256k1Fr]{
		Version: 1,
		Input:   []frontend.Variable{0},
		Output:  []frontend.Variable{0},
		Sig: circuitecdsa.Signature[emulated.Secp256k1Fr]{
			R: emulated.ValueOf[emulated.Secp256k1Fr](r),
			S: emulated.ValueOf[emulated.Secp256k1Fr](s),
		},
		Msg: emulated.ValueOf[emulated.Secp256k1Fr](hash),
		Pub: circuitecdsa.PublicKey[emulated.Secp256k1Fp, emulated.Secp256k1Fr]{
			X: emulated.ValueOf[emulated.Secp256k1Fp](privKey.PublicKey.A.X),
			Y: emulated.ValueOf[emulated.Secp256k1Fp](privKey.PublicKey.A.Y),
		},
		Sender: uints.NewU8Array(ethAddress),
	}

	err := test.IsSolved(&circuit, &circuit, ecc.BN254.ScalarField())
	if err != nil {
		return keeper.Groth16Proof{}, err
	}

	// Witness first then compile as that modifies the circuit
	witness, err := frontend.NewWitness(&circuit, ecc.BN254.ScalarField())
	if err != nil {
		return keeper.Groth16Proof{}, err
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
	if testing.Short() {
		t.Skip("Skipping ECDSA, takes a minute on my machine.")
	}

	f := initFixture(t)
	require := require.New(t)

	privKey, _ := ecdsa.GenerateKey(rand.Reader)
	proof, err := generate_ecdsa_proof(privKey)
	require.NoError(err)
	jsonproof, _ := json.Marshal(proof)

	// Register the contract
	contract := zktx.Contract{
		Verifier:    "gnark-groth16-te-BN254",
		StateDigest: []byte{0},
		ProgramId:   string(proof.VerifyingKey),
	}

	// set the contract state
	err = f.k.Contracts.Set(f.ctx, "ecdsa", contract)
	require.NoError(err)

	msg := &zktx.MsgExecuteStateChange{
		StateChanges: []*zktx.StateChange{
			&zktx.StateChange{
				ContractName: "ecdsa",
				Proof:        jsonproof,
				InitialState: []byte{0},
				FinalState:   []byte{0},
			},
		},
	}
	_, err = f.msgServer.ExecuteStateChange(f.ctx, msg)
	require.NoError(err)

	st, _ := f.k.Contracts.Get(f.ctx, "ecdsa")
	require.Equal(st.StateDigest, []byte{0})
}
