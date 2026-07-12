package cli

import (
	"encoding/json"
	"fmt"

	"github.com/spf13/cobra"
)

// mdRenderer is implemented by report views that render to Markdown.
type mdRenderer interface{ Markdown() string }

// emit writes a report view as JSON when asJSON is set, else as Markdown.
func emit(cmd *cobra.Command, asJSON bool, v mdRenderer) error {
	if asJSON {
		return emitJSON(cmd, v)
	}
	fmt.Fprint(cmd.OutOrStdout(), v.Markdown())
	return nil
}

// emitJSON marshals v as indented JSON to the command's output.
func emitJSON(cmd *cobra.Command, v any) error {
	b, err := json.MarshalIndent(v, "", "  ")
	if err != nil {
		return err
	}
	fmt.Fprintln(cmd.OutOrStdout(), string(b))
	return nil
}

// jsonFlag registers a standard --json flag bound to the target.
func jsonFlag(cmd *cobra.Command, target *bool) {
	cmd.Flags().BoolVar(target, "json", false, "emit JSON instead of Markdown")
}
