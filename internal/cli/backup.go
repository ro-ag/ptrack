package cli

import (
	"fmt"
	"os"
	"path/filepath"
	"time"

	"github.com/ro-ag/ptrack/internal/store"
	"github.com/spf13/cobra"
)

// newBackupCmd builds `ptrack backup`: copies the current project DB into the
// global backups directory, records the backup in the global store, and prints
// the resulting backup path.
func newBackupCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "backup",
		Short: "Back up the current project database",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			cwd, err := os.Getwd()
			if err != nil {
				return err
			}
			dbPath, err := store.FindProjectDB(cwd)
			if err != nil {
				return err
			}
			home, err := store.GlobalHome()
			if err != nil {
				return err
			}
			destDir := filepath.Join(home, "backups")
			ts := time.Now().Unix()
			backupPath, err := store.BackupProject(dbPath, destDir, ts)
			if err != nil {
				return err
			}
			if g, err := store.OpenGlobal(); err == nil {
				_ = g.RecordBackup(projectRoot(dbPath), backupPath)
				_ = g.Close()
			}
			fmt.Fprintln(cmd.OutOrStdout(), backupPath)
			return nil
		},
	}
	return cmd
}
