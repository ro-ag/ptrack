// Command ptrack-db-export creates one inert JSON migration stage from a
// legacy ptrack home without modifying any source database.
package main

import (
	"flag"
	"fmt"
	"io"
	"os"
)

func main() { os.Exit(run(os.Args[1:], os.Stderr)) }

func run(args []string, stderr io.Writer) int {
	flags := flag.NewFlagSet("ptrack-db-export", flag.ContinueOnError)
	flags.SetOutput(stderr)
	home := flags.String("home", "", "absolute legacy ptrack home (required)")
	output := flags.String("output", "", "absolute absent staging directory (required)")
	if err := flags.Parse(args); err != nil {
		return 2
	}
	if flags.NArg() != 0 || *home == "" || *output == "" {
		fmt.Fprintln(stderr, "--home and --output are required; positional arguments are not accepted")
		return 2
	}
	if err := ExportJSONStage(*home, *output); err != nil {
		fmt.Fprintln(stderr, err)
		return 1
	}
	return 0
}
