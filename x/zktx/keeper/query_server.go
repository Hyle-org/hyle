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

// Handler for the contract contract query method
func (qs queryServer) Contract(ctx context.Context, req *zktx.ContractRequest) (*zktx.ContractResponse, error) {
	contract, err := qs.k.Contracts.Get(ctx, req.ContractName)
	if err != nil {
		if errors.Is(err, collections.ErrNotFound) {
			return &zktx.ContractResponse{Contract: zktx.Contract{}}, nil
		}

		return nil, status.Error(codes.Internal, err.Error())
	}

	return &zktx.ContractResponse{Contract: contract}, nil
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
