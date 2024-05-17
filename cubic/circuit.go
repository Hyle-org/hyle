package main

import (
	"bytes"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"

	"github.com/consensys/gnark-crypto/ecc"
	"github.com/consensys/gnark/backend/groth16"
	"github.com/consensys/gnark/backend/witness"
	"github.com/consensys/gnark/frontend"
	"github.com/consensys/gnark/frontend/cs/r1cs"
)

type Circuit struct {
	X frontend.Variable `gnark:"x"`       // x  --> secret visibility (default)
	Y frontend.Variable `gnark:",public"` // Y  --> public visibility
}

// Define declares the circuit logic. The compiler then produces a list of constraints
// which must be satisfied (valid witness) in order to create a valid zk-SNARK
func (circuit *Circuit) Define(api frontend.API) error {
	// compute x**3 and store it in the local variable x3.
	x3 := api.Mul(circuit.X, circuit.X, circuit.X)

	// compute x**3 + x + 5 and store it in the local variable res
	res := api.Add(x3, circuit.X, 5)

	// assert that the statement x**3 + x + 5 == y is true.
	api.AssertIsEqual(circuit.Y, res)
	return nil
}

func ReadFromInputPath(pathInput string) (map[string]interface{}, error) {

	// Construct the absolute path to the file
	//absPath := filepath.Join("../", pathInput)
	absPath, err := filepath.Abs(pathInput)
	if err != nil {
		fmt.Println("Error constructing absolute path:", err)
		return nil, err
	}

	file, err := os.Open(absPath)
	if err != nil {
		panic(err)
	}
	defer file.Close()

	var data map[string]interface{}
	err = json.NewDecoder(file).Decode(&data)
	if err != nil {
		panic(err)
	}

	return data, nil
}

func FromJson(pathInput string) witness.Witness {

	data, err := ReadFromInputPath(pathInput)
	if err != nil {
		panic(err)
	}

	assignment := Circuit{
		X: frontend.Variable(data["x"].(string)),
		Y: frontend.Variable(data["y"].(string)),
	}

	w, err := frontend.NewWitness(&assignment, ecc.BN254.ScalarField())
	if err != nil {
		panic(err)
	}
	return w
}

type Groth16Proof struct {
	Proof         []byte `json:"proof"`
	VerifyingKey  []byte `json:"verifying_key"`
	PublicWitness []byte `json:"public_witness"`
}

func main() {
	// compiles our circuit into a R1CS
	var circuit Circuit
	ccs, _ := frontend.Compile(ecc.BN254.ScalarField(), r1cs.NewBuilder, &circuit)

	// groth16 zkSNARK: Setup
	pk, vk, _ := groth16.Setup(ccs)

	// witness definition
	witness := FromJson("./input.json")
	publicWitness, _ := witness.Public()

	// groth16: Prove & Verify
	proof, _ := groth16.Prove(ccs, pk, witness)

	////////////  HYLE ////////////

	// Prepare data to send it to
	var proofBuf bytes.Buffer
	proof.WriteTo(&proofBuf)

	var vkBuf bytes.Buffer
	vk.WriteTo(&vkBuf)

	var publicWitnessBuf bytes.Buffer
	publicWitness.WriteTo(&publicWitnessBuf)

	hyle_proof := Groth16Proof{
		Proof:         proofBuf.Bytes(),
		VerifyingKey:  vkBuf.Bytes(),
		PublicWitness: publicWitnessBuf.Bytes(),
	}
	hyle_proof_marshalled, _ := json.Marshal(hyle_proof)
	f, _ := os.Create("proof.json")
	f.Write(hyle_proof_marshalled)

	///////////////////////////////
}
