package keeper

import (
	"bytes"
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
	// All we need to do is drop the in-flight timeout
	// TODO: maybe we want to increment nonces of accounts or something?
	for _, tx := range txs.Txs {
		// Ignore tx already settled - we can't easily remove timeouts from the list
		// so this is done here.
		if settled, err := k.SettledTx.Has(ctx, tx); err != nil || settled {
			continue
		}
		// Remove the payload metadata as we no longer need it (this is just GC)
		for i := uint32(0); ; i++ {
			payload, err := k.ProvenPayload.Get(ctx, collections.Join(tx, i))
			if err != nil {
				break
			}
			// Update the contract transaction list
			contract, err := k.Contracts.Get(ctx, payload.ContractName)
			if err != nil {
				break
			}
			// This ought always match because they're in order.
			if bytes.Equal(contract.NextTxToSettle, tx) {
				contract.NextTxToSettle = payload.NextTxHash
			}
			if bytes.Equal(contract.LatestTxReceived, tx) {
				contract.LatestTxReceived = nil
			}
			err = k.Contracts.Set(ctx, payload.ContractName, contract)
			if err != nil {
				return err
			}

			k.ProvenPayload.Remove(ctx, collections.Join(tx, i))
		}
		err = k.SettledTx.Set(ctx, tx, false)
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
