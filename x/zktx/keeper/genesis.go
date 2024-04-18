package keeper

import (
	"context"

	"github.com/hyle/hyle/zktx"
)

// InitGenesis initializes the module state from a genesis state.
func (k *Keeper) InitGenesis(ctx context.Context, data *zktx.GenesisState) error {
	if err := k.Params.Set(ctx, data.Params); err != nil {
		return err
	}

	for contractName, contract := range data.Contracts {
		if err := k.Contracts.Set(ctx, contractName, *contract); err != nil {
			return err
		}
	}

	return nil
}

// ExportGenesis exports the module state to a genesis state.
func (k *Keeper) ExportGenesis(ctx context.Context) (*zktx.GenesisState, error) {
	params, err := k.Params.Get(ctx)
	if err != nil {
		return nil, err
	}

	// converting from collections.Map[string, zktx.Contract] to  map[string]*Contract
	contracts := make(map[string]*zktx.Contract)
	if err := k.Contracts.Walk(ctx, nil, func(contractName string, contract zktx.Contract) (bool, error) {
		contracts[contractName] = &contract
		return false, nil
	}); err != nil {
		return nil, err
	}

	return &zktx.GenesisState{
		Params:    params,
		Contracts: contracts,
	}, nil
}
