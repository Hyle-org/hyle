package module

import (
	"cosmossdk.io/core/address"
	"cosmossdk.io/core/appmodule"
	"cosmossdk.io/core/store"
	"cosmossdk.io/depinject"
	"cosmossdk.io/x/tx/signing"
	"google.golang.org/protobuf/proto"

	"github.com/cosmos/cosmos-sdk/codec"
	authtypes "github.com/cosmos/cosmos-sdk/x/auth/types"

	modulev1 "github.com/hyle-org/hyle/x/zktx/api/module/v1"
	zktxv1 "github.com/hyle-org/hyle/x/zktx/api/v1"
	"github.com/hyle-org/hyle/x/zktx/keeper"
)

var _ appmodule.AppModule = AppModule{}

// IsOnePerModuleType implements the depinject.OnePerModuleType interface.
func (am AppModule) IsOnePerModuleType() {}

// IsAppModule implements the appmodule.AppModule interface.
func (am AppModule) IsAppModule() {}

// Provide custom signers that don't really do anything
// We just want to send messages and ignore the cosmos SDK logic
// (see also skipAnteHandlers in app.toml)
func FakeRegisterSigner() signing.CustomGetSigner {
	return signing.CustomGetSigner{
		MsgType: proto.MessageName(&zktxv1.MsgRegisterContract{}),
		Fn: func(msg proto.Message) ([][]byte, error) {
			return [][]byte{[]byte("fake-signer")}, nil
		},
	}
}
func FakeExecuteSigner() signing.CustomGetSigner {
	return signing.CustomGetSigner{
		MsgType: proto.MessageName(&zktxv1.MsgExecuteStateChanges{}),
		Fn: func(msg proto.Message) ([][]byte, error) {
			return [][]byte{[]byte("fake-signer")}, nil
		},
	}
}
func FakeVerifySigner() signing.CustomGetSigner {
	return signing.CustomGetSigner{
		MsgType: proto.MessageName(&zktxv1.MsgVerifyProof{}),
		Fn: func(msg proto.Message) ([][]byte, error) {
			return [][]byte{[]byte("fake-signer")}, nil
		},
	}
}

func init() {
	appmodule.Register(
		&modulev1.Module{},
		appmodule.Provide(
			ProvideModule,
			FakeRegisterSigner,
			FakeExecuteSigner,
			FakeVerifySigner,
		),
	)
}

type ModuleInputs struct {
	depinject.In

	Cdc          codec.Codec
	StoreService store.KVStoreService
	AddressCodec address.Codec

	Config *modulev1.Module
}

type ModuleOutputs struct {
	depinject.Out

	Module appmodule.AppModule
	Keeper keeper.Keeper
}

func ProvideModule(in ModuleInputs) ModuleOutputs {
	// default to governance as authority if not provided
	authority := authtypes.NewModuleAddress("gov")
	if in.Config.Authority != "" {
		authority = authtypes.NewModuleAddressOrBech32Address(in.Config.Authority)
	}

	k := keeper.NewKeeper(in.Cdc, in.AddressCodec, in.StoreService, authority.String())
	m := NewAppModule(in.Cdc, k)

	return ModuleOutputs{
		Module: m, Keeper: k,
	}
}
