package cli

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/ro-ag/ptrack/internal/store"
	"github.com/spf13/cobra"
)

const (
	hookBegin = "# ptrack:begin"
	hookEnd   = "# ptrack:end"
	hookBody  = `command -v ptrack >/dev/null 2>&1 && ptrack commit record --sha "$(git rev-parse HEAD)" --subject "$(git log -1 --pretty=%s)" >/dev/null 2>&1 || true`
)

// newHookCmd builds `ptrack hook` with install, uninstall, and status.
func newHookCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "hook",
		Short: "Manage the git post-commit hook that auto-records commits",
	}

	install := &cobra.Command{
		Use:   "install",
		Short: "Install the post-commit hook (auto-records each commit into ptrack)",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			path, err := hookPath()
			if err != nil {
				return err
			}
			existing, err := os.ReadFile(path)
			if err != nil && !os.IsNotExist(err) {
				return err
			}
			updated, changed := upsertHook(string(existing))
			if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
				return err
			}
			if err := os.WriteFile(path, []byte(updated), 0o755); err != nil {
				return err
			}
			if changed {
				fmt.Fprintf(cmd.OutOrStdout(), "installed post-commit hook at %s\n", path)
			} else {
				fmt.Fprintln(cmd.OutOrStdout(), "post-commit hook already up to date")
			}
			return nil
		},
	}

	uninstall := &cobra.Command{
		Use:   "uninstall",
		Short: "Remove the ptrack block from the post-commit hook",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			path, err := hookPath()
			if err != nil {
				return err
			}
			data, err := os.ReadFile(path)
			if err != nil {
				if os.IsNotExist(err) {
					fmt.Fprintln(cmd.OutOrStdout(), "no post-commit hook")
					return nil
				}
				return err
			}
			stripped := stripHook(string(data))
			if strings.TrimSpace(stripped) == "#!/bin/sh" || strings.TrimSpace(stripped) == "" {
				// Nothing left but the shebang — remove the file.
				if err := os.Remove(path); err != nil {
					return err
				}
			} else if err := os.WriteFile(path, []byte(stripped), 0o755); err != nil {
				return err
			}
			fmt.Fprintln(cmd.OutOrStdout(), "removed ptrack post-commit hook")
			return nil
		},
	}

	status := &cobra.Command{
		Use:   "status",
		Short: "Report whether the post-commit hook is installed",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			path, err := hookPath()
			if err != nil {
				return err
			}
			data, err := os.ReadFile(path)
			if err == nil && strings.Contains(string(data), hookBegin) {
				fmt.Fprintf(cmd.OutOrStdout(), "installed: %s\n", path)
			} else {
				fmt.Fprintln(cmd.OutOrStdout(), "not installed (run 'ptrack hook install')")
			}
			return nil
		},
	}

	cmd.AddCommand(install, uninstall, status)
	return cmd
}

// hookPath resolves <project root>/.git/hooks/post-commit, erroring when .git is
// not a plain directory (e.g. a worktree/submodule gitlink).
func hookPath() (string, error) {
	cwd, err := os.Getwd()
	if err != nil {
		return "", err
	}
	dbPath, err := store.FindProjectDB(cwd)
	if err != nil {
		return "", err
	}
	gitDir := filepath.Join(projectRoot(dbPath), ".git")
	fi, err := os.Stat(gitDir)
	if err != nil || !fi.IsDir() {
		return "", fmt.Errorf(".git is not a directory at %s — install the hook manually", gitDir)
	}
	return filepath.Join(gitDir, "hooks", "post-commit"), nil
}

// block returns the marker-delimited managed hook block.
func hookBlock() string {
	return hookBegin + "\n" + hookBody + "\n" + hookEnd + "\n"
}

// upsertHook inserts or refreshes the ptrack block in a post-commit script,
// returning the new content and whether it changed.
func upsertHook(content string) (string, bool) {
	block := hookBlock()
	begin := strings.Index(content, hookBegin)
	end := strings.Index(content, hookEnd)
	if begin >= 0 && end > begin {
		before := content[:begin]
		after := strings.TrimPrefix(content[end+len(hookEnd):], "\n")
		out := before + block + after
		return out, out != content
	}
	if strings.TrimSpace(content) == "" {
		return "#!/bin/sh\n" + block, true
	}
	trimmed := strings.TrimRight(content, "\n")
	return trimmed + "\n\n" + block, true
}

// stripHook removes the ptrack block from a post-commit script.
func stripHook(content string) string {
	begin := strings.Index(content, hookBegin)
	end := strings.Index(content, hookEnd)
	if begin < 0 || end <= begin {
		return content
	}
	before := strings.TrimRight(content[:begin], "\n")
	after := strings.TrimPrefix(content[end+len(hookEnd):], "\n")
	if before == "" {
		return after
	}
	if after == "" {
		return before + "\n"
	}
	return before + "\n" + after
}
