package zktx

type HyleContext struct {
	Identity string
	TxHash   []byte
}

type HyleOutput struct {
	InitialState []byte `json:"initial_state"`
	NextState    []byte `json:"next_state"`
	Identity     string `json:"identity"`
	TxHash       []byte `json:"tx_hash"`
	PayloadHash  []byte `json:"payload_hash"`
	Success      bool   `json:"success"`
}
