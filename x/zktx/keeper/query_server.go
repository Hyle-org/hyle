package keeper

import (
	"context"
	"errors"

	"cosmossdk.io/collections"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"

	"github.com/hyle/hyle/zktx"
)

var _ zktx.QueryServer = queryServer{}

// NewQueryServerImpl returns an implementation of the module QueryServer.
func NewQueryServerImpl(k Keeper) zktx.QueryServer {
	return queryServer{k}
}

type queryServer struct {
	k Keeper
}

// Handler for the contract state query method
func (qs queryServer) ContractState(ctx context.Context, req *zktx.ContractStateRequest) (*zktx.ContractStateResponse, error) {
	state, err := qs.k.ContractStates.Get(ctx, req.ContractAddress)
	if err != nil {
		if errors.Is(err, collections.ErrNotFound) {
			return &zktx.ContractStateResponse{State: zktx.ContractState{}}, nil
		}

		return nil, status.Error(codes.Internal, err.Error())
	}

	return &zktx.ContractStateResponse{State: state}, nil
}

// Params defines the handler for the Query/Params RPC method.
func (qs queryServer) Params(ctx context.Context, req *zktx.QueryParamsRequest) (*zktx.QueryParamsResponse, error) {
	params, err := qs.k.Params.Get(ctx)
	if err != nil {
		if errors.Is(err, collections.ErrNotFound) {
			return &zktx.QueryParamsResponse{Params: zktx.Params{}}, nil
		}

		return nil, status.Error(codes.Internal, err.Error())
	}

	return &zktx.QueryParamsResponse{Params: params}, nil
}
