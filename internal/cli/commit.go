package cli

import (
	"fmt"
	"regexp"
	"strconv"

	"github.com/ro-ag/ptrack/internal/store"
	"github.com/spf13/cobra"
)

var taskRefRe = regexp.MustCompile(`#(\d+)`)

// parseTaskRef returns the first #<id> task reference in s, or 0.
func parseTaskRef(s string) uint64 {
	m := taskRefRe.FindStringSubmatch(s)
	if m == nil {
		return 0
	}
	id, _ := strconv.ParseUint(m[1], 10, 64)
	return id
}

// newCommitCmd builds `ptrack commit` with add, list, and record subcommands.
func newCommitCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "commit",
		Short: "Track git commits in the project audit trail",
	}

	var (
		addTask uint64
		addPlan uint64
	)
	add := &cobra.Command{
		Use:   "add <sha> <subject...>",
		Short: "Record a commit (links to --task/--plan, else the active plan)",
		Args:  cobra.MinimumNArgs(2),
		RunE: func(cmd *cobra.Command, args []string) error {
			s, err := openProject()
			if err != nil {
				return err
			}
			defer s.Close()
			planID, taskID, err := resolveCommitLink(s, addTask, addPlan)
			if err != nil {
				return err
			}
			c, err := s.AddCommit(args[0], joinArgs(args[1:]), planID, taskID)
			if err != nil {
				return err
			}
			fmt.Fprintf(cmd.OutOrStdout(), "commit %s recorded\n", short(c.SHA))
			return nil
		},
	}
	add.Flags().Uint64Var(&addTask, "task", 0, "link to this task")
	add.Flags().Uint64Var(&addPlan, "plan", 0, "link to this plan (default: active plan)")

	var (
		recSHA     string
		recSubject string
	)
	record := &cobra.Command{
		Use:   "record",
		Short: "Record HEAD from a git hook (parses #<id> from the subject)",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			if recSHA == "" {
				return fmt.Errorf("--sha is required")
			}
			s, err := openProject()
			if err != nil {
				return err
			}
			defer s.Close()
			var planID, taskID uint64
			if ref := parseTaskRef(recSubject); ref != 0 {
				if t, err := s.GetTask(ref); err == nil {
					taskID = t.ID
					planID = t.PlanID
				}
			}
			if taskID == 0 {
				if m, err := s.GetMeta(); err == nil {
					planID = m.ActivePlan
				}
			}
			if _, err := s.AddCommit(recSHA, recSubject, planID, taskID); err != nil {
				return err
			}
			return nil
		},
	}
	record.Flags().StringVar(&recSHA, "sha", "", "commit SHA")
	record.Flags().StringVar(&recSubject, "subject", "", "commit subject line")

	var (
		listTask uint64
		listPlan uint64
		listJSON bool
	)
	list := &cobra.Command{
		Use:   "list",
		Short: "List commits (optionally --task/--plan)",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			s, err := openProject()
			if err != nil {
				return err
			}
			defer s.Close()
			commits, err := s.ListCommits()
			if err != nil {
				return err
			}
			if listTask != 0 {
				commits, err = s.CommitsByTask(listTask)
			} else if listPlan != 0 {
				commits, err = s.CommitsByPlan(listPlan)
			}
			if err != nil {
				return err
			}
			if listJSON {
				return emitJSON(cmd, commits)
			}
			out := cmd.OutOrStdout()
			for _, c := range commits {
				link := ""
				if c.TaskID != 0 {
					link = fmt.Sprintf(" (task %d)", c.TaskID)
				} else if c.PlanID != 0 {
					link = fmt.Sprintf(" (plan %d)", c.PlanID)
				}
				fmt.Fprintf(out, "%s %s%s\n", short(c.SHA), c.Subject, link)
			}
			return nil
		},
	}
	list.Flags().Uint64Var(&listTask, "task", 0, "only commits linked to this task")
	list.Flags().Uint64Var(&listPlan, "plan", 0, "only commits linked to this plan")
	jsonFlag(list, &listJSON)

	cmd.AddCommand(add, record, list)
	return cmd
}

// resolveCommitLink picks the plan/task link for a manual commit add.
func resolveCommitLink(s *store.Store, taskID, planID uint64) (uint64, uint64, error) {
	if taskID != 0 {
		t, err := s.GetTask(taskID)
		if err != nil {
			return 0, 0, err
		}
		return t.PlanID, t.ID, nil
	}
	if planID != 0 {
		return planID, 0, nil
	}
	m, err := s.GetMeta()
	if err != nil {
		return 0, 0, err
	}
	return m.ActivePlan, 0, nil
}

func short(sha string) string {
	if len(sha) > 8 {
		return sha[:8]
	}
	return sha
}
