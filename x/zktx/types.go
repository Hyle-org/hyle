package zktx

type HyleContext struct {
	Origin    string
	Caller    string
	BlockTime uint64
	BlockNb   uint64
	TxHash    []byte
}

type HyleOutput struct {
	InitialState []byte `json:"initial_state"`
	NextState    []byte `json:"next_state"`
	Origin       string `json:"origin"`
	Caller       string `json:"caller"`
	BlockNumber  uint64 `json:"block_number"`
	BlockTime    uint64 `json:"block_time"`
	TxHash       []byte `json:"tx_hash"`
}
