package cli

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"

	"github.com/ro-ag/ptrack/internal/capability"
	"github.com/ro-ag/ptrack/internal/store"
	"github.com/spf13/cobra"
)

// newCapabilityCmd builds the capability management and broker bridge surface.
func newCapabilityCmd() *cobra.Command {
	command := &cobra.Command{
		Use:   "capability",
		Short: "Manage and invoke explicit project host capabilities",
	}
	command.AddCommand(newCapabilityCallCmd(), newCapabilityMCPCmd())
	return command
}

func newCapabilityCallCmd() *cobra.Command {
	var arguments string
	command := &cobra.Command{
		Use:   "call <tool>",
		Short: "Call a capability tool through the active host broker",
		Args:  cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			if !json.Valid([]byte(arguments)) {
				return errors.New("--arguments must be one JSON object")
			}
			var object map[string]any
			if err := json.Unmarshal([]byte(arguments), &object); err != nil || object == nil {
				return errors.New("--arguments must be one JSON object")
			}
			client, token, err := activeCapabilityClient()
			if err != nil {
				return err
			}
			result, err := client.Call(cmd.Context(), token, capability.ToolCall{
				Name: args[0], Arguments: json.RawMessage(arguments),
			})
			if err != nil {
				return err
			}
			fmt.Fprintln(cmd.OutOrStdout(), string(result))
			return nil
		},
	}
	command.Flags().StringVar(&arguments, "arguments", "{}", "JSON object matching the tool input schema")
	return command
}

func newCapabilityMCPCmd() *cobra.Command {
	return &cobra.Command{
		Use:   "mcp",
		Short: "Serve provider-compatible MCP tools over stdio",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			client, token, err := activeCapabilityClient()
			if err != nil {
				return err
			}
			return capability.ServeMCP(cmd.Context(), cmd.InOrStdin(), cmd.OutOrStdout(), func(ctx context.Context, call capability.ToolCall) (json.RawMessage, error) {
				return client.Call(ctx, token, call)
			})
		},
	}
}

func activeCapabilityClient() (capability.BrokerClient, string, error) {
	token := os.Getenv("PTRACK_CAPABILITY_TOKEN")
	if token == "" {
		return capability.BrokerClient{}, "", errors.New("capability broker token is unavailable; launch this command from an agent terminal in p-track")
	}
	cwd, err := os.Getwd()
	if err != nil {
		return capability.BrokerClient{}, "", err
	}
	dbPath, err := store.FindProjectDB(cwd)
	if err != nil {
		return capability.BrokerClient{}, "", err
	}
	root := filepath.Dir(filepath.Dir(dbPath))
	root, err = filepath.EvalSymlinks(root)
	if err != nil {
		return capability.BrokerClient{}, "", err
	}
	home, err := store.GlobalHome()
	if err != nil {
		return capability.BrokerClient{}, "", err
	}
	client, err := capability.ClientForProject(home, root)
	if err != nil {
		return capability.BrokerClient{}, "", err
	}
	if err := capability.ValidateSessionEnvironment(client.Descriptor); err != nil {
		return capability.BrokerClient{}, "", err
	}
	return client, token, nil
}
