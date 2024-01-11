package keeper

import (
	"context"
	"errors"
	"fmt"

	"cosmossdk.io/collections"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"

	"github.com/cosmos/cosmos-sdk/types/query"
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

// Counter defines the handler for the Query/Counter RPC method.
func (qs queryServer) Counter(ctx context.Context, req *zktx.QueryCounterRequest) (*zktx.QueryCounterResponse, error) {
	if _, err := qs.k.addressCodec.StringToBytes(req.Address); err != nil {
		return nil, fmt.Errorf("invalid sender address: %w", err)
	}

	counter, err := qs.k.Counter.Get(ctx, req.Address)
	if err != nil {
		if errors.Is(err, collections.ErrNotFound) {
			return &zktx.QueryCounterResponse{Counter: 0}, nil
		}

		return nil, status.Error(codes.Internal, err.Error())
	}

	return &zktx.QueryCounterResponse{Counter: counter}, nil
}

// Counters defines the handler for the Query/Counters RPC method.
func (qs queryServer) Counters(ctx context.Context, req *zktx.QueryCountersRequest) (*zktx.QueryCountersResponse, error) {
	counters, pageRes, err := query.CollectionPaginate(
		ctx,
		qs.k.Counter,
		req.Pagination,
		func(key string, value uint64) (*zktx.Counter, error) {
			return &zktx.Counter{
				Address: key,
				Count:   value,
			}, nil
		})
	if err != nil {
		return nil, err
	}

	return &zktx.QueryCountersResponse{Counters: counters, Pagination: pageRes}, nil
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
