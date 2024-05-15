package module

import (
	"encoding/base64"
	"fmt"
	"os"

	"github.com/cosmos/cosmos-sdk/client"
	"github.com/cosmos/cosmos-sdk/client/tx"
	"github.com/hyle/hyle/zktx"
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

	txCmd.AddCommand(&cobra.Command{
		Use:   "execute [sender] [[contract_name] [proof] [initial_state] [final_state]]...",
		Short: "Execute a state change",
		Args:  cobra.MinimumNArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			clientCtx, err := client.GetClientTxContext(cmd)
			if err != nil {
				return err
			}

			if len(args[1:])%4 != 0 {
				return fmt.Errorf("invalid state changes")
			}

			stateChanges := make([]*zktx.StateChange, 0, len(args[1:])/4)
			for i := 0; i < len(args[1:]); i += 4 {
				proof, err := os.ReadFile(args[i+1])
				if err != nil {
					return err
				}
				initialState, err := base64.StdEncoding.DecodeString(args[i+2])
				if err != nil {
					return err
				}
				finalState, err := base64.StdEncoding.DecodeString(args[i+3])
				if err != nil {
					return err
				}
				stateChanges = append(stateChanges, &zktx.StateChange{
					ContractName: args[i],
					Proof:        proof,
					InitialState: initialState,
					FinalState:   finalState,
				})
			}

			msg := &zktx.MsgExecuteStateChange{
				Sender:       clientCtx.GetFromAddress().String(),
				StateChanges: stateChanges,
			}

			return tx.GenerateOrBroadcastTxCLI(clientCtx, cmd.Flags(), msg)
		},
	})

	return txCmd
}
