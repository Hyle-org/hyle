package module

import (
	"encoding/base64"
	"encoding/hex"
	"errors"
	"fmt"
	"os"
	"strconv"

	"github.com/cosmos/cosmos-sdk/client"
	"github.com/cosmos/cosmos-sdk/client/tx"
	"github.com/hyle-org/hyle/x/zktx"
	"github.com/spf13/cobra"
)

// Copied from cosmos SDK client/v2/autocli/flag/binary.go
func GetBinaryValue(s string) ([]byte, error) {
	if data, err := os.ReadFile(s); err == nil {
		return data, nil
	}

	if data, err := hex.DecodeString(s); err == nil {
		return data, nil
	}

	if data, err := base64.StdEncoding.DecodeString(s); err == nil {
		return data, nil
	}

	return nil, errors.New("input string is neither a valid file path, hex, or base64 encoded")
}

// Update the "from" address to a random key in keyring
// This is just to make the CLI work, the node actually skips checks.
func UpdateFromAddress(clientCtx *client.Context) error {
	krs, _ := clientCtx.Keyring.List()
	if len(krs) == 0 {
		return fmt.Errorf("no keyring found")
	}
	clientCtx.From = krs[0].Name
	clientCtx.FromAddress, _ = krs[0].GetAddress()
	clientCtx.FromName = krs[0].Name
	return nil
}

func (am AppModule) GetTxCmd() *cobra.Command {
	txCmd := &cobra.Command{
		Use:   zktx.ModuleName,
		Short: "Hyle transaction subcommands",
		RunE:  client.ValidateCmd,
	}

	txCmd.PersistentFlags().Int("account-number", 0, "Account number of the contract")
	txCmd.PersistentFlags().Int("sequence", 0, "Sequence number of the contract")

	txCmd.AddCommand(&cobra.Command{
		Use:   "publish [identity] [data] [[contract_name]]...",
		Short: "Publish a payload for a number of contract",
		Args:  cobra.MinimumNArgs(3),
		RunE: func(cmd *cobra.Command, args []string) error {
			clientCtx, err := client.GetClientTxContext(cmd)
			if err != nil {
				return err
			}

			if err := UpdateFromAddress(&clientCtx); err != nil {
				return err
			}

			if len(args[2:]) == 0 {
				return fmt.Errorf("no contract specified")
			}

			data, err := GetBinaryValue(args[1])
			if err != nil {
				return err
			}

			contractsList := make([]string, len(args[2:]))
			for i := 2; i < 2+len(contractsList); i++ {
				contractName := args[i]
				contractsList = append(contractsList, contractName)
			}
			payloads := &zktx.Payloads{
				ContractsName: contractsList,
				Data:          data,
			}

			msg := &zktx.MsgPublishPayloads{
				Identity: args[0],
				Payloads: payloads,
			}
			// Need to set them to something for offline mode.
			cmd.Flags().Set("account-number", "0")
			cmd.Flags().Set("sequence", "0")
			return tx.GenerateOrBroadcastTxCLI(clientCtx.WithOffline(true), cmd.Flags(), msg)
		},
	})

	txCmd.AddCommand(&cobra.Command{
		Use:   "prove [tx_hash] [index] [contract_name] [proof]",
		Short: "Publish a proof for a payload",
		Args:  cobra.MinimumNArgs(4),
		RunE: func(cmd *cobra.Command, args []string) error {
			clientCtx, err := client.GetClientTxContext(cmd)

			if err != nil {
				return err
			}

			if err := UpdateFromAddress(&clientCtx); err != nil {
				return err
			}

			if len(args) != 4 {
				return fmt.Errorf("expected 4 arguments, got %d", len(args))
			}

			tx_hash, err := GetBinaryValue(args[0])
			if err != nil {
				return err
			}

			index, err := strconv.Atoi(args[1])
			if err != nil {
				return err
			}

			contract_name := args[2]

			proof, err := GetBinaryValue(args[3])
			if err != nil {
				return err
			}

			msg := &zktx.MsgPublishPayloadProof{
				TxHash:       tx_hash,
				PayloadIndex: uint32(index),
				ContractName: contract_name,
				Proof:        proof,
			}
			// Need to set them to something for offline mode.
			cmd.Flags().Set("account-number", "0")
			cmd.Flags().Set("sequence", "0")
			return tx.GenerateOrBroadcastTxCLI(clientCtx.WithOffline(true), cmd.Flags(), msg)
		},
	})

	txCmd.AddCommand(&cobra.Command{
		Use:   "register [owner] [verifier] [program_id] [contract_name] [state_digest]",
		Short: "Register a contract",
		Args:  cobra.MinimumNArgs(5),
		RunE: func(cmd *cobra.Command, args []string) error {
			clientCtx, err := client.GetClientTxContext(cmd)
			if err != nil {
				return err
			}
			if err := UpdateFromAddress(&clientCtx); err != nil {
				return err
			}

			programId, err := GetBinaryValue(args[2])
			if err != nil {
				return err
			}
			StateDigest, err := GetBinaryValue(args[4])
			if err != nil {
				return err
			}
			msg := &zktx.MsgRegisterContract{
				Owner:        args[0],
				Verifier:     args[1],
				ProgramId:    programId,
				ContractName: args[3],
				StateDigest:  StateDigest,
			}
			// Need to set them to something for offline mode.
			cmd.Flags().Set("account-number", "0")
			cmd.Flags().Set("sequence", "0")
			return tx.GenerateOrBroadcastTxCLI(clientCtx.WithOffline(true), cmd.Flags(), msg)
		},
	})

	return txCmd
}
