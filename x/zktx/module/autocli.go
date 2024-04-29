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
					RpcMethod: "Contract",
					Use:       "contract [contract_name]",
					Short:     "Get the current state of a contract",
					PositionalArgs: []*autocliv1.PositionalArgDescriptor{
						{ProtoField: "contract_name"},
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
					Use:       "execute [contract_name] [proof] [initial_state] [final_state]",
					Short:     "Permissionless state transition for a contract.",
					PositionalArgs: []*autocliv1.PositionalArgDescriptor{
						{ProtoField: "contract_name"},
						{ProtoField: "proof"},
						{ProtoField: "initial_state"},
						{ProtoField: "final_state"},
					},
				},
				{
					RpcMethod: "VerifyProof",
					Use:       "verify [contract_name] [proof]",
					Short:     "Verify a zero knowledge proof in a stateless manner.",
					PositionalArgs: []*autocliv1.PositionalArgDescriptor{
						{ProtoField: "contract_name"},
						{ProtoField: "proof"},
					},
				},
				{
					RpcMethod: "RegisterContract",
					Use:       "register [owner] [contract_name] [verifier] [program_id] [state_digest]",
					Short:     "Register a new smart contract.",
					PositionalArgs: []*autocliv1.PositionalArgDescriptor{
						{ProtoField: "owner"},
						{ProtoField: "contract_name"},
						{ProtoField: "verifier"},
						{ProtoField: "program_id"},
						{ProtoField: "state_digest"},
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
