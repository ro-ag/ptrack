package cli

import (
	"errors"
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

	var (
		listPlan  uint64
		listTask  uint64
		listLimit int
		listJSON  bool
	)
	list := &cobra.Command{
		Use:   "list",
		Short: "List notes, newest first (scope with --plan/--task, bound with --limit)",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			if listPlan != 0 && listTask != 0 {
				return errors.New("--plan and --task are mutually exclusive")
			}
			s, err := openProject()
			if err != nil {
				return err
			}
			defer s.Close()

			var notes []model.Note
			switch {
			case listTask != 0:
				notes, err = s.NotesByTask(listTask)
			case listPlan != 0:
				notes, err = s.NotesByPlan(listPlan)
			default:
				notes, err = s.ListNotes()
			}
			if err != nil {
				return err
			}
			notes = newestFirst(notes, listLimit)

			if listJSON {
				type noteRow struct {
					ID       uint64 `json:"id"`
					Target   string `json:"target"`
					TargetID uint64 `json:"target_id"`
					Body     string `json:"body"`
				}
				rows := make([]noteRow, 0, len(notes))
				for _, n := range notes {
					rows = append(rows, noteRow{n.ID, string(n.Target), n.TargetID, n.Body})
				}
				return emitJSON(cmd, rows)
			}
			out := cmd.OutOrStdout()
			for _, n := range notes {
				if n.TargetID == 0 {
					fmt.Fprintf(out, "#%d (%s) %s\n", n.ID, n.Target, n.Body)
				} else {
					fmt.Fprintf(out, "#%d (%s #%d) %s\n", n.ID, n.Target, n.TargetID, n.Body)
				}
			}
			return nil
		},
	}
	list.Flags().Uint64Var(&listPlan, "plan", 0, "only notes attached to this plan")
	list.Flags().Uint64Var(&listTask, "task", 0, "only notes attached to this task")
	list.Flags().IntVar(&listLimit, "limit", 20, "max notes to show (0 = all)")
	jsonFlag(list, &listJSON)

	cmd.AddCommand(add, list)
	return cmd
}

// newestFirst reverses notes (stored oldest-first) and caps to limit (0 = all).
func newestFirst(notes []model.Note, limit int) []model.Note {
	out := make([]model.Note, 0, len(notes))
	for i := len(notes) - 1; i >= 0; i-- {
		if limit > 0 && len(out) >= limit {
			break
		}
		out = append(out, notes[i])
	}
	return out
}
