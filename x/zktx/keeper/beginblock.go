package keeper

import (
	"errors"

	"cosmossdk.io/collections"
	sdk "github.com/cosmos/cosmos-sdk/types"
)

func BeginBlocker(ctx sdk.Context, k Keeper) error {
	height := ctx.BlockHeight()
	txs, err := k.Timeout.Get(ctx, height)
	if errors.Is(err, collections.ErrNotFound) {
		return nil
	}
	if err != nil {
		return err
	}
	// All we need to do is drop the in-flight payload
	// TODO: maybe we want to increment nonces of accounts or something?
	for _, tx := range txs.Payloads {
		err = k.ProvenPayload.Remove(ctx, collections.Join(tx.TxHash, tx.PayloadIndex)) // noop if already processed / not found
		if err != nil {
			return err
		}
	}
	return nil
}
