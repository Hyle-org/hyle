package zktx

import "cosmossdk.io/collections"

const ModuleName = "zktx"

var (
	ParamsKey         = collections.NewPrefix(0)
	CounterKey        = collections.NewPrefix(1)
	ContractStatesKey = collections.NewPrefix(2)
)
