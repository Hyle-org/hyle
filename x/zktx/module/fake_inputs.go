package module

import (
	"context"

	abci "github.com/cometbft/cometbft/abci/types"
	sdk "github.com/cosmos/cosmos-sdk/types"
)

type FakeAccountKeeper struct {
}

func (k FakeAccountKeeper) NewAccount(context.Context, sdk.AccountI) sdk.AccountI {
	return nil
}
func (k FakeAccountKeeper) SetAccount(context.Context, sdk.AccountI) {
}
func (k FakeAccountKeeper) IterateAccounts(ctx context.Context, process func(sdk.AccountI) (stop bool)) {
}

type FakeStakingKeeper struct {
}

func (k FakeStakingKeeper) ApplyAndReturnValidatorSetUpdates(context.Context) (updates []abci.ValidatorUpdate, err error) {
	return nil, nil
}
