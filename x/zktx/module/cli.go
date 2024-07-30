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

	txCmd.AddCommand(&cobra.Command{
		Use:   "publish [[contract_name] [payload]]...",
		Short: "Publish a number of payloads",
		Args:  cobra.MinimumNArgs(2),
		RunE: func(cmd *cobra.Command, args []string) error {
			clientCtx, err := client.GetClientTxContext(cmd)

			if err != nil {
				return err
			}

			if err := UpdateFromAddress(&clientCtx); err != nil {
				return err
			}

			if len(args)%2 != 0 {
				return fmt.Errorf("invalid state changes")
			}

			payloads := make([]*zktx.Payload, 0, len(args)/2)
			for i := 0; i < len(args); i += 2 {
				data, err := GetBinaryValue(args[i+1])
				if err != nil {
					return err
				}
				payloads = append(payloads, &zktx.Payload{
					ContractName: args[i],
					Data:         data,
				})
			}

			msg := &zktx.MsgPublishPayloads{
				Payloads: payloads,
			}

			return tx.GenerateOrBroadcastTxCLI(clientCtx, cmd.Flags(), msg)
		},
	})

	txCmd.AddCommand(&cobra.Command{
		Use:   "prove [tx_hash] [index] [contract_name] [payload] [proof]",
		Short: "Publish a proof for a payload",
		Args:  cobra.MinimumNArgs(5),
		RunE: func(cmd *cobra.Command, args []string) error {
			clientCtx, err := client.GetClientTxContext(cmd)

			if err != nil {
				return err
			}

			if err := UpdateFromAddress(&clientCtx); err != nil {
				return err
			}

			if len(args) != 5 {
				return fmt.Errorf("expected 5 arguments, got %d", len(args))
			}

			tx_hash, err := GetBinaryValue(args[0])
			if err != nil {
				return err
			}

			index, err := strconv.Atoi(args[1])
			if err != nil {
				return err
			}

			data, err := GetBinaryValue(args[3])
			if err != nil {
				return err
			}
			payload := &zktx.Payload{
				ContractName: args[2],
				Data:         data,
			}

			proof, err := GetBinaryValue(args[4])
			if err != nil {
				return err
			}

			msg := &zktx.MsgPublishPayloadProof{
				TxHash:       tx_hash,
				PayloadIndex: uint32(index),
				Payload: 	  payload,
				Proof:        proof,
			}

			return tx.GenerateOrBroadcastTxCLI(clientCtx, cmd.Flags(), msg)
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

			return tx.GenerateOrBroadcastTxCLI(clientCtx, cmd.Flags(), msg)
		},
	})

	return txCmd
}
