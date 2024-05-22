package module

import (
	"fmt"
	"os"

	"github.com/cosmos/cosmos-sdk/client"
	"github.com/cosmos/cosmos-sdk/client/flags"
	"github.com/cosmos/cosmos-sdk/client/tx"
	"github.com/hyle-org/hyle/zktx"
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
		Use:   "execute [hyle_sender] [[contract_name] [proof]]... --from [cosmos address]",
		Short: "Execute a state change",
		Args:  cobra.MinimumNArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			clientCtx, err := client.GetClientTxContext(cmd)
			if err != nil {
				return err
			}

			if len(args[1:])%2 != 0 {
				return fmt.Errorf("invalid state changes")
			}

			stateChanges := make([]*zktx.StateChange, 0, len(args[1:])/2)
			for i := 1; i < len(args); i += 2 {
				proof, err := os.ReadFile(args[i+1])
				if err != nil {
					return err
				}
				stateChanges = append(stateChanges, &zktx.StateChange{
					ContractName: args[i],
					Proof:        proof,
				})
			}

			msg := &zktx.MsgExecuteStateChange{
				Sender:       clientCtx.GetFromAddress().String(),
				HyleSender:   args[0],
				BlockTime:    0,
				BlockNb:      0,
				TxHash:       []byte("TODO"),
				StateChanges: stateChanges,
			}

			return tx.GenerateOrBroadcastTxCLI(clientCtx, cmd.Flags(), msg)
		},
	})

	return txCmd
}
