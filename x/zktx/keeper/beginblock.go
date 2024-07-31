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
		if has, err := k.ProvenPayload.Has(ctx, collections.Join(tx.TxHash, tx.PayloadIndex)); !has || err != nil {
			continue
		}
		err = k.ProvenPayload.Remove(ctx, collections.Join(tx.TxHash, tx.PayloadIndex))
		if err != nil {
			return err
		}
		err = k.SettledTx.Set(ctx, tx.TxHash, false) // TODO: not efficient to do it several time but meh.
		if err != nil {
			return err
		}
	}
	err = k.Timeout.Remove(ctx, height)
	if err != nil {
		return err
	}
	return nil
}
