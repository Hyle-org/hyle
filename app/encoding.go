package app

import (
	"github.com/cosmos/cosmos-sdk/std"
	"github.com/julienrbrt/chain-minimal/app/params"
)

// MakeEncodingConfig creates an EncodingConfig. This function
// should be used only in tests or when creating a new app instance (NewApp*()).
// App user shouldn't create new codecs - use the app.AppCodec instead.
func MakeEncodingConfig() params.EncodingConfig {
	encodingConfig := params.MakeEncodingConfig()
	std.RegisterLegacyAminoCodec(encodingConfig.Amino)
	std.RegisterInterfaces(encodingConfig.InterfaceRegistry)
	ModuleBasics.RegisterLegacyAminoCodec(encodingConfig.Amino)
	ModuleBasics.RegisterInterfaces(encodingConfig.InterfaceRegistry)
	return encodingConfig
}
