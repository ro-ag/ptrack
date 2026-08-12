// Command db-migrate-export exports one explicitly named legacy bbolt database
// into a checksummed migration bundle without modifying the source.
package main

import (
	"flag"
	"fmt"
	"io"
	"os"

	"github.com/ro-ag/ptrack/internal/store"
)

func main() {
	os.Exit(run(os.Args[1:], os.Stderr))
}

func run(args []string, stderr io.Writer) int {
	flags := flag.NewFlagSet("db-migrate-export", flag.ContinueOnError)
	flags.SetOutput(stderr)
	kindValue := flags.String("kind", "", "source database kind: project or global (required)")
	sourcePath := flags.String("source", "", "absolute path to the source bbolt database (required)")
	outputPath := flags.String("output", "", "absolute path for the new migration bundle (required)")
	if err := flags.Parse(args); err != nil {
		return 2
	}
	if flags.NArg() != 0 {
		fmt.Fprintln(stderr, "positional arguments are not accepted")
		return 2
	}
	if *kindValue == "" || *sourcePath == "" || *outputPath == "" {
		fmt.Fprintln(stderr, "--kind, --source, and --output are required")
		return 2
	}
	kind, err := store.ParseMigrationKind(*kindValue)
	if err != nil {
		fmt.Fprintln(stderr, err)
		return 2
	}
	if err := store.ExportMigrationBundle(kind, *sourcePath, *outputPath); err != nil {
		fmt.Fprintln(stderr, err)
		return 1
	}
	return 0
}
