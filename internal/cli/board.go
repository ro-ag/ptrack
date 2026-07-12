package cli

import (
	"errors"

	"github.com/ro-ag/ptrack/internal/report"
	"github.com/spf13/cobra"
)

// newBoardCmd builds `ptrack board`: a kanban view of a plan's tasks grouped by
// status. Defaults to the active plan; --plan selects another.
func newBoardCmd() *cobra.Command {
	var (
		asJSON bool
		planID uint64
	)
	cmd := &cobra.Command{
		Use:   "board",
		Short: "Kanban board of a plan's tasks (todo/doing/blocked/done)",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			s, err := openProject()
			if err != nil {
				return err
			}
			defer s.Close()
			if planID == 0 {
				m, err := s.GetMeta()
				if err != nil {
					return err
				}
				if m.ActivePlan == 0 {
					return errors.New("no active plan; set one with 'ptrack plan use <id>' or pass --plan")
				}
				planID = m.ActivePlan
			}
			v, err := report.BoardFor(s, planID)
			if err != nil {
				return err
			}
			return emit(cmd, asJSON, v)
		},
	}
	cmd.Flags().Uint64Var(&planID, "plan", 0, "plan id (default: active plan)")
	jsonFlag(cmd, &asJSON)
	return cmd
}
