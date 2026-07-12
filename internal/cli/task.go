package cli

import (
	"errors"
	"fmt"

	"github.com/ro-ag/ptrack/internal/model"
	"github.com/spf13/cobra"
)

// newTaskCmd builds `ptrack task` with add, list, start, done, and block
// subcommands.
func newTaskCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "task",
		Short: "Manage tasks",
	}

	add := &cobra.Command{
		Use:   "add <title...>",
		Short: "Create a new todo task (defaults to the active plan)",
		Args:  cobra.MinimumNArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			planID, err := cmd.Flags().GetUint64("plan")
			if err != nil {
				return err
			}
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
			t, err := s.AddTask(planID, joinArgs(args))
			if err != nil {
				return err
			}
			fmt.Fprintf(cmd.OutOrStdout(), "task #%d %s (plan %d)\n", t.ID, t.Title, t.PlanID)
			return nil
		},
	}
	add.Flags().Uint64("plan", 0, "plan id to add the task to (defaults to the active plan)")

	list := &cobra.Command{
		Use:   "list",
		Short: "List tasks (all, or filtered by --plan)",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			planID, err := cmd.Flags().GetUint64("plan")
			if err != nil {
				return err
			}
			s, err := openProject()
			if err != nil {
				return err
			}
			defer s.Close()
			var tasks []model.Task
			if planID != 0 {
				tasks, err = s.ListTasksByPlan(planID)
			} else {
				tasks, err = s.ListTasks()
			}
			if err != nil {
				return err
			}
			out := cmd.OutOrStdout()
			for _, t := range tasks {
				fmt.Fprintf(out, "#%d [%s] %s (plan %d)\n", t.ID, t.Status, t.Title, t.PlanID)
			}
			return nil
		},
	}
	list.Flags().Uint64("plan", 0, "only list tasks of this plan")

	start := &cobra.Command{
		Use:   "start <id>",
		Short: "Mark a task in progress (doing)",
		Args:  cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			return setTaskStatus(args[0], model.TaskDoing)
		},
	}

	done := &cobra.Command{
		Use:   "done <id>",
		Short: "Mark a task done",
		Args:  cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			return setTaskStatus(args[0], model.TaskDone)
		},
	}

	block := &cobra.Command{
		Use:   "block <id>",
		Short: "Mark a task blocked",
		Args:  cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			return setTaskStatus(args[0], model.TaskBlocked)
		},
	}

	cmd.AddCommand(add, list, start, done, block)
	return cmd
}

// setTaskStatus opens the project, parses the id, and applies the given status.
func setTaskStatus(arg string, st model.TaskStatus) error {
	id, err := parseID(arg)
	if err != nil {
		return err
	}
	s, err := openProject()
	if err != nil {
		return err
	}
	defer s.Close()
	return s.SetTaskStatus(id, st)
}
