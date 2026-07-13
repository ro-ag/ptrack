package cli

// NoProjectHint is the friendly guidance shown when `ptrack` is run with no
// subcommand outside any ptrack project, instead of a bare error.
func NoProjectHint() string {
	return "No ptrack project here yet.\n\n" +
		"  ptrack init                 create one in this directory (or the git root)\n" +
		"  ptrack init --goal \"...\"     create one and set the goal\n" +
		"  ptrack --help               list all commands\n\n" +
		"Once a project exists, run `ptrack` to open the dashboard.\n"
}
