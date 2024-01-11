package module

import (
	"cosmossdk.io/core/address"
	"cosmossdk.io/core/appmodule"
	"cosmossdk.io/core/store"
	"cosmossdk.io/depinject"

	"github.com/cosmos/cosmos-sdk/codec"
	authtypes "github.com/cosmos/cosmos-sdk/x/auth/types"

	modulev1 "github.com/hyle/hyle/zktx/api/module/v1"
	"github.com/hyle/hyle/zktx/keeper"
)

var _ appmodule.AppModule = AppModule{}

// IsOnePerModuleType implements the depinject.OnePerModuleType interface.
func (am AppModule) IsOnePerModuleType() {}

// IsAppModule implements the appmodule.AppModule interface.
func (am AppModule) IsAppModule() {}

func init() {
	appmodule.Register(
		&modulev1.Module{},
		appmodule.Provide(ProvideModule),
	)
}

type ModuleInputs struct {
	depinject.In

	Cdc          codec.Codec
	StoreService store.KVStoreService
	AddressCodec address.Codec

	Config *modulev1.Module
}

type (
	// ValidatorAddressCodec is an alias for address.Codec for validator addresses.
	ValidatorAddressCodec address.Codec

	// ConsensusAddressCodec is an alias for address.Codec for validator consensus addresses.
	ConsensusAddressCodec address.Codec
)

type ModuleOutputs struct {
	depinject.Out

	Module appmodule.AppModule
	Keeper keeper.Keeper

	FakeAccountKeeper FakeAccountKeeper
	FakeStakingKeeper FakeStakingKeeper

	//AddressCodecFactory func() address.Codec
	//ValidatorAddressCodecFactory func() ValidatorAddressCodec
	//ConsensusAddressCodecFactory func() ConsensusAddressCodec
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
		FakeAccountKeeper: FakeAccountKeeper{},
		FakeStakingKeeper: FakeStakingKeeper{},
		//AddressCodecFactory: func() address.Codec { return addresscodec.NewBech32Codec("toto") },
		//ValidatorAddressCodecFactory: func() ValidatorAddressCodec { return addresscodec.NewBech32Codec("toto") },
		//ConsensusAddressCodecFactory: func() ConsensusAddressCodec { return addresscodec.NewBech32Codec("toto") },
	}
}
