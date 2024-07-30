package keeper

import (
	"errors"

	"cosmossdk.io/collections"
	sdk "github.com/cosmos/cosmos-sdk/types"
	"github.com/hyle-org/hyle/x/zktx"
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
		// TODO: we could conceptually just delete this, but it makes it quite annoying to do indexation
		err = k.ProvenPayload.Set(ctx, collections.Join(tx.TxHash, tx.PayloadIndex), zktx.PayloadMetadata{
			Verified: true,
		}) // noop if already processed / not found
		if err != nil {
			return err
		}
	}
	return nil
}
