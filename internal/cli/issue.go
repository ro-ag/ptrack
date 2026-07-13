package cli

import (
	"fmt"

	"github.com/ro-ag/ptrack/internal/model"
	"github.com/ro-ag/ptrack/internal/report"
	"github.com/spf13/cobra"
)

// newIssueCmd builds `ptrack issue` with add, list, show, close, open, and
// severity subcommands.
func newIssueCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "issue",
		Short: "Manage issues (tracked problems or bugs)",
	}

	var (
		severity string
		taskID   uint64
		body     string
	)
	add := &cobra.Command{
		Use:   "add <title...>",
		Short: "Create a new open issue",
		Args:  cobra.MinimumNArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			sev, err := parseSeverity(severity)
			if err != nil {
				return err
			}
			s, err := openProject()
			if err != nil {
				return err
			}
			defer s.Close()
			is, err := s.AddIssue(joinArgs(args), body, sev, taskID)
			if err != nil {
				return err
			}
			fmt.Fprintf(cmd.OutOrStdout(), "issue #%d [%s] %s\n", is.ID, is.Severity, is.Title)
			return nil
		},
	}
	add.Flags().StringVar(&severity, "severity", "", "severity: low, medium (default), high, critical")
	add.Flags().Uint64Var(&taskID, "task", 0, "link the issue to this task")
	add.Flags().StringVar(&body, "body", "", "longer description")

	var (
		listJSON   bool
		statusFilt string
	)
	list := &cobra.Command{
		Use:   "list",
		Short: "List issues (optionally --status open|closed)",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			var want model.IssueStatus
			switch statusFilt {
			case "":
			case "open":
				want = model.IssueOpen
			case "closed":
				want = model.IssueClosed
			default:
				return fmt.Errorf("invalid --status %q (want open or closed)", statusFilt)
			}
			s, err := openProject()
			if err != nil {
				return err
			}
			defer s.Close()
			issues, err := s.ListIssues()
			if err != nil {
				return err
			}
			if want != "" {
				filtered := issues[:0:0]
				for _, is := range issues {
					if is.Status == want {
						filtered = append(filtered, is)
					}
				}
				issues = filtered
			}
			if listJSON {
				return emitJSON(cmd, issues)
			}
			out := cmd.OutOrStdout()
			for _, is := range issues {
				link := ""
				if is.TaskID != 0 {
					link = fmt.Sprintf(" (task %d)", is.TaskID)
				}
				fmt.Fprintf(out, "#%d [%s] %s %s%s\n", is.ID, is.Severity, is.Status, is.Title, link)
			}
			return nil
		},
	}
	list.Flags().StringVar(&statusFilt, "status", "", "filter by status: open or closed")
	jsonFlag(list, &listJSON)

	var showJSON bool
	show := &cobra.Command{
		Use:   "show <id>",
		Short: "Show an issue with its linked task",
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
			v, err := report.ShowIssue(s, id)
			if err != nil {
				return err
			}
			return emit(cmd, showJSON, v)
		},
	}
	jsonFlag(show, &showJSON)

	closeCmd := &cobra.Command{
		Use:   "close <id>",
		Short: "Close an issue",
		Args:  cobra.ExactArgs(1),
		RunE:  issueStatusSetter(model.IssueClosed),
	}
	openCmd := &cobra.Command{
		Use:   "open <id>",
		Short: "Reopen an issue",
		Args:  cobra.ExactArgs(1),
		RunE:  issueStatusSetter(model.IssueOpen),
	}

	sev := &cobra.Command{
		Use:   "severity <id> <low|medium|high|critical>",
		Short: "Set an issue's severity",
		Args:  cobra.ExactArgs(2),
		RunE: func(cmd *cobra.Command, args []string) error {
			id, err := parseID(args[0])
			if err != nil {
				return err
			}
			s2, err := parseSeverity(args[1])
			if err != nil {
				return err
			}
			s, err := openProject()
			if err != nil {
				return err
			}
			defer s.Close()
			return s.SetIssueSeverity(id, s2)
		},
	}

	cmd.AddCommand(add, list, show, closeCmd, openCmd, sev)
	return cmd
}

// parseSeverity validates a severity string; empty returns "" (store defaults it).
func parseSeverity(s string) (model.Severity, error) {
	switch s {
	case "":
		return "", nil
	case "low", "medium", "high", "critical":
		return model.Severity(s), nil
	default:
		return "", fmt.Errorf("invalid severity %q (want low, medium, high, critical)", s)
	}
}

func issueStatusSetter(st model.IssueStatus) func(*cobra.Command, []string) error {
	return func(cmd *cobra.Command, args []string) error {
		id, err := parseID(args[0])
		if err != nil {
			return err
		}
		s, err := openProject()
		if err != nil {
			return err
		}
		defer s.Close()
		return s.SetIssueStatus(id, st)
	}
}
