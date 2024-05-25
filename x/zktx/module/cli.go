package module

import (
	"fmt"
	"os"

	"github.com/cosmos/cosmos-sdk/client"
	"github.com/cosmos/cosmos-sdk/client/flags"
	"github.com/cosmos/cosmos-sdk/client/tx"
	"github.com/hyle-org/hyle/x/zktx"
	"github.com/spf13/cobra"
)

func (am AppModule) GetTxCmd() *cobra.Command {
	txCmd := &cobra.Command{
		Use:   zktx.ModuleName,
		Short: "Hyle transaction subcommands",
		//DisableFlagParsing:         true,
		//SuggestionsMinimumDistance: 2,
		RunE: client.ValidateCmd,
	}

	txCmd.PersistentFlags().String(flags.FlagFrom, "", "Name or address of private key with which to sign")

	txCmd.AddCommand(&cobra.Command{
		Use:   "execute [[contract_name] [proof]]... --from [cosmos address]",
		Short: "Execute a number of state changes",
		Args:  cobra.MinimumNArgs(2),
		RunE: func(cmd *cobra.Command, args []string) error {
			clientCtx, err := client.GetClientTxContext(cmd)
			if err != nil {
				return err
			}

			if len(args)%2 != 0 {
				return fmt.Errorf("invalid state changes")
			}

			stateChanges := make([]*zktx.StateChange, 0, len(args)/2)
			for i := 0; i < len(args); i += 2 {
				proof, err := os.ReadFile(args[i+1])
				if err != nil {
					return err
				}
				stateChanges = append(stateChanges, &zktx.StateChange{
					ContractName: args[i],
					Proof:        proof,
				})
			}

			msg := &zktx.MsgExecuteStateChanges{
				Sender:       clientCtx.GetFromAddress().String(),
				StateChanges: stateChanges,
			}

			return tx.GenerateOrBroadcastTxCLI(clientCtx, cmd.Flags(), msg)
		},
	})

	return txCmd
}
