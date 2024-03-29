package zktx

import "cosmossdk.io/collections"

const ModuleName = "zktx"

var (
	ParamsKey         = collections.NewPrefix(0)
	ContractStatesKey = collections.NewPrefix(1)
)
