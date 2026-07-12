package cli

import (
	"fmt"

	"github.com/ro-ag/ptrack/internal/model"
	"github.com/spf13/cobra"
)

// newNoteCmd builds `ptrack note add <text...>`: attaches a note to the project
// by default, or to a specific task or plan when --task/--plan is given.
func newNoteCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "note",
		Short: "Manage notes",
	}

	add := &cobra.Command{
		Use:   "add <text...>",
		Short: "Add a note to the project, a plan, or a task",
		Args:  cobra.MinimumNArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			taskID, err := cmd.Flags().GetUint64("task")
			if err != nil {
				return err
			}
			planID, err := cmd.Flags().GetUint64("plan")
			if err != nil {
				return err
			}
			var (
				target   model.NoteTarget
				targetID uint64
			)
			switch {
			case taskID != 0:
				target = model.TargetTask
				targetID = taskID
			case planID != 0:
				target = model.TargetPlan
				targetID = planID
			default:
				target = model.TargetProject
			}
			s, err := openProject()
			if err != nil {
				return err
			}
			defer s.Close()
			n, err := s.AddNote(target, targetID, joinArgs(args))
			if err != nil {
				return err
			}
			fmt.Fprintf(cmd.OutOrStdout(), "note #%d %s\n", n.ID, n.Body)
			return nil
		},
	}
	add.Flags().Uint64("task", 0, "attach the note to this task")
	add.Flags().Uint64("plan", 0, "attach the note to this plan")

	cmd.AddCommand(add)
	return cmd
}
