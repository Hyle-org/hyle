package keeper

import (
	"context"
	"errors"

	"cosmossdk.io/collections"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"

	"github.com/hyle-org/hyle/zktx"
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

// Handler for the contract contract query method
func (qs queryServer) ContractList(ctx context.Context, req *zktx.ContractListRequest) (*zktx.ContractListResponse, error) {
	contractList := make([]string, 0)
	qs.k.Contracts.Walk(ctx, nil, func(key string, contract zktx.Contract) (stop bool, err error) {
		contractList = append(contractList, key)
		return false, nil
	})
	return &zktx.ContractListResponse{Contracts: contractList}, nil
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
