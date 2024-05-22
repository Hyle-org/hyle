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
	Version   frontend.Variable   `gnark:",public"`
	InputLen  frontend.Variable   `gnark:",public"`
	Input     []frontend.Variable `gnark:",public"`
	OutputLen frontend.Variable   `gnark:",public"`
	Output    []frontend.Variable `gnark:",public"`
	SenderLen frontend.Variable   `gnark:",public"`
	Sender    []uints.U8          `gnark:",public"`
	CallerLen frontend.Variable   `gnark:",public"`
	Caller    []uints.U8          `gnark:",public"`
	BlockTime frontend.Variable   `gnark:",public"`
	BlockNb   frontend.Variable   `gnark:",public"`
	TxHashLen frontend.Variable   `gnark:",public"`
	TxHash    []uints.U8          `gnark:",public"`
}

func (c *HyleCircuit) Define(api frontend.API) error { return nil }

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

func parseArray(input *fr.Vector) ([]byte, error) {
	length := (*input)[0].Uint64()
	if length == 0 {
		*input = (*input)[1:]
		return []byte{}, nil
	}
	// Sanity check
	if length > uint64(len(*input)) {
		return nil, fmt.Errorf("array length exceeds input size")
	}
	output := make([]byte, length)
	for i := uint64(0); i < length; i++ {
		output[i] = uint8((*input)[i+1].Uint64())
	}
	// Consume the input
	*input = (*input)[length+1:]
	return output, nil
}

func parseString(input *fr.Vector) (string, error) {
	if output, err := parseArray(input); err != nil {
		return "", err
	} else {
		return string(output), nil
	}
}

func parseNumber[T uint8 | uint16 | uint32 | uint64](input *fr.Vector) (T, error) {
	if len(*input) < 1 {
		return 0, fmt.Errorf("input is empty")
	}
	val := T((*input)[0].Uint64())
	*input = (*input)[1:]
	return val, nil
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
	if output.InitialState, err = parseArray(&slice); err != nil {
		return nil, err
	}
	if output.NextState, err = parseArray(&slice); err != nil {
		return nil, err
	}
	if output.Sender, err = parseString(&slice); err != nil {
		return nil, err
	}
	if output.Caller, err = parseString(&slice); err != nil {
		return nil, err
	}
	if output.BlockNumber, err = parseNumber[uint64](&slice); err != nil {
		return nil, err
	}
	if output.BlockTime, err = parseNumber[uint64](&slice); err != nil {
		return nil, err
	}
	if output.TxHash, err = parseArray(&slice); err != nil {
		return nil, err
	}

	return output, nil
}
