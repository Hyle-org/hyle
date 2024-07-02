package gnark

import (
	"bytes"
	"fmt"

	"github.com/consensys/gnark-crypto/ecc"
	"github.com/consensys/gnark-crypto/ecc/bn254/fr"
	tedwards "github.com/consensys/gnark-crypto/ecc/twistededwards"
	"github.com/consensys/gnark/backend/groth16"
	"github.com/consensys/gnark/backend/witness"
	"github.com/consensys/gnark/frontend"
	"github.com/consensys/gnark/std/algebra/native/twistededwards"
	"github.com/consensys/gnark/std/math/uints"

	"github.com/hyle-org/hyle/x/zktx"
)

// This is the public interface that a verifiable circuit must implement
type HyleCircuit struct {
	Version     frontend.Variable   `gnark:",public"`
	InputLen    frontend.Variable   `gnark:",public"`
	Input       []frontend.Variable `gnark:",public"`
	OutputLen   frontend.Variable   `gnark:",public"`
	Output      []frontend.Variable `gnark:",public"`
	IdentityLen frontend.Variable   `gnark:",public"` // This is encoded as a single ASCII character per byte
	Identity    []frontend.Variable `gnark:",public"` // The max capacity is 256 bytes (arbitrarily)
	TxHash      [64]uints.U8        `gnark:",public"`
}

func (c *HyleCircuit) Define(api frontend.API) error { return nil }

// Utility for tests mostly
func ToArray256(d []byte) [256]uints.U8 {
	// Pad to 256
	if len(d) < 256 {
		padded := make([]byte, 256)
		copy(padded, d)
		d = padded
	}
	return [256]uints.U8(uints.NewU8Array(d))
}

func ToArray64(d []byte) [64]uints.U8 {
	// Pad to 64
	if len(d) < 64 {
		padded := make([]byte, 64)
		copy(padded, d)
		d = padded
	}
	return [64]uints.U8(uints.NewU8Array(d))
}

// Struct type expected for the "proof" argument
type Groth16Proof struct {
	Proof         []byte `json:"proof"`
	VerifyingKey  []byte `json:"verifying_key"`
	PublicWitness []byte `json:"public_witness"`
}

func (proof *Groth16Proof) ParseProof() (groth16.Proof, groth16.VerifyingKey, witness.Witness, error) {
	proofReader := bytes.NewReader(proof.Proof)
	g16p := groth16.NewProof(ecc.BN254)
	if _, err := g16p.ReadFrom(proofReader); err != nil {
		return nil, nil, nil, fmt.Errorf("failed to parse groth16 proof: %s", err)
	}

	proofReader = bytes.NewReader(proof.VerifyingKey)
	vk := groth16.NewVerifyingKey(ecc.BN254)
	if _, err := vk.ReadFrom(proofReader); err != nil {
		return nil, nil, nil, fmt.Errorf("failed to parse groth16 vk: %w", err)
	}

	proofReader = bytes.NewReader(proof.PublicWitness)
	fid, _ := twistededwards.GetSnarkField(tedwards.BN254) // Note: handle the error if required
	witness, err := witness.New(fid)
	if err != nil {
		return nil, nil, nil, fmt.Errorf("failed to initialize groth16 witness: %w", err)
	}

	if _, err := witness.ReadFrom(proofReader); err != nil {
		return nil, nil, nil, fmt.Errorf("failed to parse groth16 witness: %w", err)
	}

	return g16p, vk, witness, nil
}

func parseArray(input *fr.Vector, length int) ([]byte, error) {
	output := make([]byte, length)
	for i := 0; i < length; i++ {
		output[i] = uint8((*input)[i].Uint64())
	}
	// Consume the input
	*input = (*input)[length:]
	return output, nil
}

func parseSlice(input *fr.Vector) ([]byte, error) {
	length := (*input)[0].Uint64()
	if length == 0 {
		*input = (*input)[1:]
		return []byte{}, nil
	}
	*input = (*input)[1:]
	// Sanity check
	if length > 0x1000000 || length > uint64(len(*input)) {
		return nil, fmt.Errorf("array length exceeds input size")
	}
	return parseArray(input, int(length))
}

func parseString(input *fr.Vector) (string, error) {
	if output, err := parseSlice(input); err != nil {
		return "", err
	} else {
		return string(output), nil
	}
}

func (proof *Groth16Proof) ExtractData(witness witness.Witness) (*zktx.HyleOutput, error) {
	// Check payload version identifier
	pubWitVector, ok := witness.Vector().(fr.Vector)
	if !ok {
		return nil, fmt.Errorf("failed to cast witness vector to fr.Vector")
	} else if pubWitVector[0] != fr.NewElement(1) {
		return nil, fmt.Errorf("invalid version identifier %s, expected 1", pubWitVector[0].Text(10))
	}
	output := &zktx.HyleOutput{}

	// Manually parse the circuit.
	var err error
	slice := pubWitVector[1:]
	if output.InitialState, err = parseSlice(&slice); err != nil {
		return nil, err
	}
	if output.NextState, err = parseSlice(&slice); err != nil {
		return nil, err
	}
	if output.Origin, err = parseString(&slice); err != nil {
		return nil, err
	}
	// Skip remaining bytes
	slice = slice[256-len(output.Origin):]
	if output.TxHash, err = parseArray(&slice, 64); err != nil {
		return nil, err
	}

	return output, nil
}
