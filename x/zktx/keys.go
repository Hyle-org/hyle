package zktx

import "cosmossdk.io/collections"

const ModuleName = "zktx"

var (
	ParamsKey       = collections.NewPrefix(0)
	ContractNameKey = collections.NewPrefix(1)
)
