package zktx

// HyleOutput is the public values a proof is providing.
type HyleOutput struct {
	InitialState []byte `json:"initial_state"`
	NextState    []byte `json:"next_state"`
	Identity     string `json:"identity"`
	TxHash       []byte `json:"tx_hash"`
	Index        uint32 `json:"index"`
	Payloads     []byte `json:"payloads"`
	Success      bool   `json:"success"`
}
