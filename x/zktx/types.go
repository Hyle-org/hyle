package zktx

type HyleContext struct {
	Sender    string
	Caller    string
	BlockTime uint64
	BlockNb   uint64
	TxHash    []byte
}
