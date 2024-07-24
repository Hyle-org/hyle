// Code generated by protoc-gen-go-grpc. DO NOT EDIT.
// versions:
// - protoc-gen-go-grpc v1.3.0
// - protoc             (unknown)
// source: hyle/zktx/v1/tx.proto

package zktxv1

import (
	context "context"
	grpc "google.golang.org/grpc"
	codes "google.golang.org/grpc/codes"
	status "google.golang.org/grpc/status"
)

// This is a compile-time assertion to ensure that this generated file
// is compatible with the grpc package it is being compiled against.
// Requires gRPC-Go v1.32.0 or later.
const _ = grpc.SupportPackageIsVersion7

const (
	Msg_PublishPayloads_FullMethodName     = "/hyle.zktx.v1.Msg/PublishPayloads"
	Msg_PublishPayloadProof_FullMethodName = "/hyle.zktx.v1.Msg/PublishPayloadProof"
	Msg_RegisterContract_FullMethodName    = "/hyle.zktx.v1.Msg/RegisterContract"
)

// MsgClient is the client API for Msg service.
//
// For semantics around ctx use and closing/ending streaming RPCs, please refer to https://pkg.go.dev/google.golang.org/grpc/?tab=doc#ClientConn.NewStream.
type MsgClient interface {
	// execute a zk-proven state change
	PublishPayloads(ctx context.Context, in *MsgPublishPayloads, opts ...grpc.CallOption) (*MsgPublishPayloadsResponse, error)
	// Verify a payload
	PublishPayloadProof(ctx context.Context, in *MsgPublishPayloadProof, opts ...grpc.CallOption) (*MsgPublishPayloadProofResponse, error)
	// RegisterContract registers a contract
	RegisterContract(ctx context.Context, in *MsgRegisterContract, opts ...grpc.CallOption) (*MsgRegisterContractResponse, error)
}

type msgClient struct {
	cc grpc.ClientConnInterface
}

func NewMsgClient(cc grpc.ClientConnInterface) MsgClient {
	return &msgClient{cc}
}

func (c *msgClient) PublishPayloads(ctx context.Context, in *MsgPublishPayloads, opts ...grpc.CallOption) (*MsgPublishPayloadsResponse, error) {
	out := new(MsgPublishPayloadsResponse)
	err := c.cc.Invoke(ctx, Msg_PublishPayloads_FullMethodName, in, out, opts...)
	if err != nil {
		return nil, err
	}
	return out, nil
}

func (c *msgClient) PublishPayloadProof(ctx context.Context, in *MsgPublishPayloadProof, opts ...grpc.CallOption) (*MsgPublishPayloadProofResponse, error) {
	out := new(MsgPublishPayloadProofResponse)
	err := c.cc.Invoke(ctx, Msg_PublishPayloadProof_FullMethodName, in, out, opts...)
	if err != nil {
		return nil, err
	}
	return out, nil
}

func (c *msgClient) RegisterContract(ctx context.Context, in *MsgRegisterContract, opts ...grpc.CallOption) (*MsgRegisterContractResponse, error) {
	out := new(MsgRegisterContractResponse)
	err := c.cc.Invoke(ctx, Msg_RegisterContract_FullMethodName, in, out, opts...)
	if err != nil {
		return nil, err
	}
	return out, nil
}

// MsgServer is the server API for Msg service.
// All implementations must embed UnimplementedMsgServer
// for forward compatibility
type MsgServer interface {
	// execute a zk-proven state change
	PublishPayloads(context.Context, *MsgPublishPayloads) (*MsgPublishPayloadsResponse, error)
	// Verify a payload
	PublishPayloadProof(context.Context, *MsgPublishPayloadProof) (*MsgPublishPayloadProofResponse, error)
	// RegisterContract registers a contract
	RegisterContract(context.Context, *MsgRegisterContract) (*MsgRegisterContractResponse, error)
	mustEmbedUnimplementedMsgServer()
}

// UnimplementedMsgServer must be embedded to have forward compatible implementations.
type UnimplementedMsgServer struct {
}

func (UnimplementedMsgServer) PublishPayloads(context.Context, *MsgPublishPayloads) (*MsgPublishPayloadsResponse, error) {
	return nil, status.Errorf(codes.Unimplemented, "method PublishPayloads not implemented")
}
func (UnimplementedMsgServer) PublishPayloadProof(context.Context, *MsgPublishPayloadProof) (*MsgPublishPayloadProofResponse, error) {
	return nil, status.Errorf(codes.Unimplemented, "method PublishPayloadProof not implemented")
}
func (UnimplementedMsgServer) RegisterContract(context.Context, *MsgRegisterContract) (*MsgRegisterContractResponse, error) {
	return nil, status.Errorf(codes.Unimplemented, "method RegisterContract not implemented")
}
func (UnimplementedMsgServer) mustEmbedUnimplementedMsgServer() {}

// UnsafeMsgServer may be embedded to opt out of forward compatibility for this service.
// Use of this interface is not recommended, as added methods to MsgServer will
// result in compilation errors.
type UnsafeMsgServer interface {
	mustEmbedUnimplementedMsgServer()
}

func RegisterMsgServer(s grpc.ServiceRegistrar, srv MsgServer) {
	s.RegisterService(&Msg_ServiceDesc, srv)
}

func _Msg_PublishPayloads_Handler(srv interface{}, ctx context.Context, dec func(interface{}) error, interceptor grpc.UnaryServerInterceptor) (interface{}, error) {
	in := new(MsgPublishPayloads)
	if err := dec(in); err != nil {
		return nil, err
	}
	if interceptor == nil {
		return srv.(MsgServer).PublishPayloads(ctx, in)
	}
	info := &grpc.UnaryServerInfo{
		Server:     srv,
		FullMethod: Msg_PublishPayloads_FullMethodName,
	}
	handler := func(ctx context.Context, req interface{}) (interface{}, error) {
		return srv.(MsgServer).PublishPayloads(ctx, req.(*MsgPublishPayloads))
	}
	return interceptor(ctx, in, info, handler)
}

func _Msg_PublishPayloadProof_Handler(srv interface{}, ctx context.Context, dec func(interface{}) error, interceptor grpc.UnaryServerInterceptor) (interface{}, error) {
	in := new(MsgPublishPayloadProof)
	if err := dec(in); err != nil {
		return nil, err
	}
	if interceptor == nil {
		return srv.(MsgServer).PublishPayloadProof(ctx, in)
	}
	info := &grpc.UnaryServerInfo{
		Server:     srv,
		FullMethod: Msg_PublishPayloadProof_FullMethodName,
	}
	handler := func(ctx context.Context, req interface{}) (interface{}, error) {
		return srv.(MsgServer).PublishPayloadProof(ctx, req.(*MsgPublishPayloadProof))
	}
	return interceptor(ctx, in, info, handler)
}

func _Msg_RegisterContract_Handler(srv interface{}, ctx context.Context, dec func(interface{}) error, interceptor grpc.UnaryServerInterceptor) (interface{}, error) {
	in := new(MsgRegisterContract)
	if err := dec(in); err != nil {
		return nil, err
	}
	if interceptor == nil {
		return srv.(MsgServer).RegisterContract(ctx, in)
	}
	info := &grpc.UnaryServerInfo{
		Server:     srv,
		FullMethod: Msg_RegisterContract_FullMethodName,
	}
	handler := func(ctx context.Context, req interface{}) (interface{}, error) {
		return srv.(MsgServer).RegisterContract(ctx, req.(*MsgRegisterContract))
	}
	return interceptor(ctx, in, info, handler)
}

// Msg_ServiceDesc is the grpc.ServiceDesc for Msg service.
// It's only intended for direct use with grpc.RegisterService,
// and not to be introspected or modified (even as a copy)
var Msg_ServiceDesc = grpc.ServiceDesc{
	ServiceName: "hyle.zktx.v1.Msg",
	HandlerType: (*MsgServer)(nil),
	Methods: []grpc.MethodDesc{
		{
			MethodName: "PublishPayloads",
			Handler:    _Msg_PublishPayloads_Handler,
		},
		{
			MethodName: "PublishPayloadProof",
			Handler:    _Msg_PublishPayloadProof_Handler,
		},
		{
			MethodName: "RegisterContract",
			Handler:    _Msg_RegisterContract_Handler,
		},
	},
	Streams:  []grpc.StreamDesc{},
	Metadata: "hyle/zktx/v1/tx.proto",
}
