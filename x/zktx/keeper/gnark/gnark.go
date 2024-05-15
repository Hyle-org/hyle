package gnark

import (
	"bytes"
	"encoding/binary"
	"fmt"
	"strings"

	"github.com/consensys/gnark-crypto/ecc"
	"github.com/consensys/gnark-crypto/ecc/bn254/fr"
	tedwards "github.com/consensys/gnark-crypto/ecc/twistededwards"
	"github.com/consensys/gnark/backend/groth16"
	"github.com/consensys/gnark/backend/witness"
	"github.com/consensys/gnark/frontend"
	"github.com/consensys/gnark/std/algebra/native/twistededwards"
	"github.com/consensys/gnark/std/math/uints"

	"github.com/hyle/hyle/zktx"
)

// This is the public interface that a verifiable circuit must implement
type HyleCircuit struct {
	Version   frontend.Variable   `gnark:",public"`
	Input     []frontend.Variable `gnark:",public"`
	Output    []frontend.Variable `gnark:",public"`
	Sender    []uints.U8          `gnark:",public"`
	Caller    []uints.U8          `gnark:",public"`
	BlockTime frontend.Variable   `gnark:",public"`
	BlockNb   frontend.Variable   `gnark:",public"`
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

func (proof *Groth16Proof) ValidateWitnessData(hyleContext *zktx.HyleContext, msg *zktx.StateChange, witness witness.Witness) error {
	// Check payload version identifier
	pubWitVector, ok := witness.Vector().(fr.Vector)
	if !ok {
		return fmt.Errorf("failed to cast witness vector to fr.Vector")
	} else if pubWitVector[0] != fr.NewElement(1) {
		return fmt.Errorf("invalid version identifier %s, expected 1", pubWitVector[0].Text(10))
	}

	payloadSender := hyleContext.Sender
	// If we are the first state change, we need to extract the sender from the sender field
	if hyleContext.Caller == "" {
		// First, we expect to only get the part without the contract name
		index := strings.LastIndex(hyleContext.Sender, ".")
		if index == -1 {
			return fmt.Errorf("invalid sender format")
		}
		payloadSender = hyleContext.Sender[0:index]

		startOffset := 44 + len(msg.InitialState) + len(msg.FinalState)
		// Check lengths preventing panics
		if startOffset+32*len(payloadSender) > len(proof.PublicWitness) {
			return fmt.Errorf("witness data is too short for sender")
		}
		senderUints := proof.PublicWitness[startOffset : startOffset+32*len(payloadSender)]

		// Parse into string
		senderBytes := make([]byte, len(payloadSender))
		for i := 0; i < len(payloadSender); i++ {
			senderBytes[i] = byte(binary.BigEndian.Uint16(senderUints[i*32+30 : i*32+32]))
		}
		if string(senderBytes) != payloadSender {
			return fmt.Errorf("sender does not match, expected %s, got %s", payloadSender, string(senderBytes))
		}
	}

	// Extracting witness data is quite annoying and serialization formats vary.
	// The approach here is to serialize our own witness, and then ensure that this matches.
	// The actual witness can contain other data, so we skip the number of arguments.
	messageWitness, err := frontend.NewWitness(&HyleCircuit{
		Version:   1,
		Input:     []frontend.Variable{0}, // Initialised to garbage to avoid warnings inside gnark
		Output:    []frontend.Variable{0}, // Initialised to garbage to avoid warnings inside gnark
		Sender:    uints.NewU8Array([]byte(payloadSender)),
		Caller:    uints.NewU8Array([]byte(hyleContext.Caller)),
		BlockTime: hyleContext.BlockTime,
		BlockNb:   hyleContext.BlockNb,
		TxHash:    uints.NewU8Array(hyleContext.TxHash),
	}, ecc.BN254.ScalarField())
	if err != nil {
		return fmt.Errorf("failed to create message witness: %w", err)
	}

	var buffer bytes.Buffer
	messageWitness.WriteTo(&buffer)

	// Skip over the number of arguments, which is 3 u32s (public args, private args, and public args again in the vector serialization)
	// Check the version field (32 bytes for this curve)
	if !bytes.Equal(buffer.Bytes()[12:44], proof.PublicWitness[12:44]) {
		return fmt.Errorf("version data does not match")
	}

	// Match the states
	if !bytes.Equal(msg.InitialState, proof.PublicWitness[44:44+len(msg.InitialState)]) {
		return fmt.Errorf("incorrect initial state")
	}
	if !bytes.Equal(msg.FinalState, proof.PublicWitness[44+len(msg.InitialState):44+len(msg.InitialState)+len(msg.FinalState)]) {
		return fmt.Errorf("incorrect final state")
	}

	// Check lengths to avoid panics
	if buffer.Len()-64+len(msg.InitialState)+len(msg.FinalState) > len(proof.PublicWitness) {
		return fmt.Errorf("witness data is too short")
	}

	// Match the rest of the message witness, ignore the end of the public witness (additional app data)
	if !bytes.Equal(buffer.Bytes()[108:], proof.PublicWitness[44+len(msg.InitialState)+len(msg.FinalState):buffer.Len()-64+len(msg.InitialState)+len(msg.FinalState)]) {
		return fmt.Errorf("witness data does not match")
	}

	return nil
}
