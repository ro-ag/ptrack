package cli

import (
	"fmt"
	"os"
	"os/exec"
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

	var showStat bool
	show := &cobra.Command{
		Use:   "show <id|sha>",
		Short: "Show a tracked commit's diff (via git show)",
		Args:  cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			cwd, err := os.Getwd()
			if err != nil {
				return err
			}
			dbPath, err := store.FindProjectDB(cwd)
			if err != nil {
				return err
			}
			s, err := store.Open(dbPath)
			if err != nil {
				return err
			}
			ref := resolveCommitRef(s, args[0])
			s.Close()

			gitArgs := []string{"-C", projectRoot(dbPath), "show"}
			if showStat {
				gitArgs = append(gitArgs, "--stat")
			}
			gitArgs = append(gitArgs, ref)
			git := exec.Command("git", gitArgs...)
			git.Stdout = cmd.OutOrStdout()
			git.Stderr = os.Stderr
			return git.Run()
		},
	}
	show.Flags().BoolVar(&showStat, "stat", false, "show only the diffstat (changed files)")

	cmd.AddCommand(add, record, list, show)
	return cmd
}

// resolveCommitRef maps a ptrack commit id to its SHA when arg is a known id;
// otherwise it returns arg unchanged (treated as a git ref).
func resolveCommitRef(s *store.Store, arg string) string {
	if id, err := strconv.ParseUint(arg, 10, 64); err == nil {
		if commits, err := s.ListCommits(); err == nil {
			for _, c := range commits {
				if c.ID == id {
					return c.SHA
				}
			}
		}
	}
	return arg
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
