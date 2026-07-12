package cli

import (
	"errors"
	"fmt"
	"strings"

	"github.com/ro-ag/ptrack/internal/model"
	"github.com/ro-ag/ptrack/internal/report"
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

	var (
		listJSON  bool
		statusCSV string
	)
	list := &cobra.Command{
		Use:   "list",
		Short: "List tasks (all, or filtered by --plan and/or --status)",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			planID, err := cmd.Flags().GetUint64("plan")
			if err != nil {
				return err
			}
			wanted, err := parseStatusSet(statusCSV)
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
			tasks = filterByStatus(tasks, wanted)
			if listJSON {
				type taskRow struct {
					ID     uint64 `json:"id"`
					PlanID uint64 `json:"plan_id"`
					Title  string `json:"title"`
					Status string `json:"status"`
				}
				rows := make([]taskRow, 0, len(tasks))
				for _, t := range tasks {
					rows = append(rows, taskRow{t.ID, t.PlanID, t.Title, string(t.Status)})
				}
				return emitJSON(cmd, rows)
			}
			out := cmd.OutOrStdout()
			for _, t := range tasks {
				fmt.Fprintf(out, "#%d [%s] %s (plan %d)\n", t.ID, t.Status, t.Title, t.PlanID)
			}
			return nil
		},
	}
	list.Flags().Uint64("plan", 0, "only list tasks of this plan")
	list.Flags().StringVar(&statusCSV, "status", "", "filter by status (comma-separated: todo,doing,done,blocked)")
	jsonFlag(list, &listJSON)

	var showJSON bool
	show := &cobra.Command{
		Use:   "show <id>",
		Short: "Show a task with its plan and notes",
		Args:  cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			id, err := parseID(args[0])
			if err != nil {
				return err
			}
			s, err := openProject()
			if err != nil {
				return err
			}
			defer s.Close()
			v, err := report.ShowTask(s, id)
			if err != nil {
				return err
			}
			return emit(cmd, showJSON, v)
		},
	}
	jsonFlag(show, &showJSON)

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

	cmd.AddCommand(add, list, show, start, done, block)
	return cmd
}

// parseStatusSet parses a comma-separated status filter into a set. An empty
// string means "no filter" (nil set).
func parseStatusSet(csv string) (map[model.TaskStatus]bool, error) {
	if strings.TrimSpace(csv) == "" {
		return nil, nil
	}
	valid := map[model.TaskStatus]bool{
		model.TaskTodo: true, model.TaskDoing: true, model.TaskDone: true, model.TaskBlocked: true,
	}
	set := map[model.TaskStatus]bool{}
	for _, part := range strings.Split(csv, ",") {
		st := model.TaskStatus(strings.TrimSpace(part))
		if !valid[st] {
			return nil, fmt.Errorf("invalid status %q (want todo,doing,done,blocked)", part)
		}
		set[st] = true
	}
	return set, nil
}

// filterByStatus keeps only tasks whose status is in the set; a nil set keeps all.
func filterByStatus(tasks []model.Task, set map[model.TaskStatus]bool) []model.Task {
	if set == nil {
		return tasks
	}
	out := tasks[:0:0]
	for _, t := range tasks {
		if set[t.Status] {
			out = append(out, t)
		}
	}
	return out
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
