package zktx

import "cosmossdk.io/collections"

const ModuleName = "zktx"

var (
	ParamsKey        = collections.NewPrefix(0)
	ContractNameKey  = collections.NewPrefix(1)
	ProvenPayloadKey = collections.NewPrefix(2)
	TimeoutKey       = collections.NewPrefix(3)
	SettledTxKey     = collections.NewPrefix(4)
)
