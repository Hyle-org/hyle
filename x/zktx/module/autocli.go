package module

import (
	autocliv1 "cosmossdk.io/api/cosmos/autocli/v1"
	hylev1 "github.com/hyle/hyle/zktx/api/v1"
)

// AutoCLIOptions implements the autocli.HasAutoCLIConfig interface.
func (am AppModule) AutoCLIOptions() *autocliv1.ModuleOptions {
	return &autocliv1.ModuleOptions{
		Query: &autocliv1.ServiceCommandDescriptor{
			Service: hylev1.Query_ServiceDesc.ServiceName,
			RpcCommandOptions: []*autocliv1.RpcCommandOptions{
				{
					RpcMethod: "ContractState",
					Use:       "contract [contract_address]",
					Short:     "Get the current state of a contract",
					PositionalArgs: []*autocliv1.PositionalArgDescriptor{
						{ProtoField: "contract_address"},
					},
				},
				{
					RpcMethod: "Counter",
					Use:       "counter [address]",
					Short:     "Get the current value of the counter for an address",
					PositionalArgs: []*autocliv1.PositionalArgDescriptor{
						{ProtoField: "address"},
					},
				},
				{
					RpcMethod: "Params",
					Use:       "params",
					Short:     "Get the current module parameters",
				},
			},
		},
		Tx: &autocliv1.ServiceCommandDescriptor{
			Service: hylev1.Msg_ServiceDesc.ServiceName,
			RpcCommandOptions: []*autocliv1.RpcCommandOptions{
				{
					RpcMethod: "ExecuteStateChange",
					Use:       "execute [contract_address] [proof] [initial_state] [final_state]",
					Short:     "TODO",
					PositionalArgs: []*autocliv1.PositionalArgDescriptor{
						{ProtoField: "contract_address"},
						{ProtoField: "proof"},
						{ProtoField: "initial_state"},
						{ProtoField: "final_state"},
					},
				},
				{
					RpcMethod: "RegisterContract",
					Use:       "register [owner] [verifier] [program_id] [state_digest]",
					Short:     "TODO",
					PositionalArgs: []*autocliv1.PositionalArgDescriptor{
						{ProtoField: "owner"},
						{ProtoField: "verifier"},
						{ProtoField: "program_id"},
						{ProtoField: "state_digest"},
					},
				},
				{
					RpcMethod: "IncrementCounter",
					Use:       "counter [sender]",
					Short:     "Increments the counter by 1 for the sender",
					PositionalArgs: []*autocliv1.PositionalArgDescriptor{
						{ProtoField: "sender"},
					},
				},
				{
					RpcMethod: "UpdateParams",
					Skip:      true, // This is a authority gated tx, so we skip it.
				},
			},
		},
	}
}
