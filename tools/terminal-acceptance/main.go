// Command terminal-acceptance provides deterministic, content-free fixtures for
// p-track's interactive terminal acceptance matrix. It is a development tool;
// it is not embedded in or invoked by the application.
package main

import (
	"bytes"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"os/exec"
	"runtime"
	"sort"
	"strconv"
	"strings"
	"unicode/utf8"

	tea "github.com/charmbracelet/bubbletea"
)

const (
	maximumOutputMiB = 1024
	outputBlockBytes = 64 * 1024
)

var inventoryPrograms = []string{
	"agy", "bash", "claude", "cmd.exe", "codex", "fish", "gemini",
	"less", "nvim", "opencode", "powershell.exe", "pwsh", "vim", "zsh",
}

func main() {
	if err := run(os.Args[1:], os.Stdout, os.Stderr); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(2)
	}
}

func run(arguments []string, stdout, stderr io.Writer) error {
	if len(arguments) == 0 {
		return usage(stderr)
	}
	switch arguments[0] {
	case "inventory":
		return writeInventory(stdout, exec.LookPath, os.Getenv)
	case "render":
		return writeRenderFixture(stdout)
	case "output":
		flags := flag.NewFlagSet("output", flag.ContinueOnError)
		flags.SetOutput(stderr)
		mib := flags.Int("mib", 100, "MiB of deterministic output (1-1024)")
		if err := flags.Parse(arguments[1:]); err != nil {
			return err
		}
		if flags.NArg() != 0 {
			return errors.New("output accepts only the --mib flag")
		}
		return writeOutputFixture(stdout, *mib)
	case "interactive":
		if len(arguments) != 1 {
			return errors.New("interactive accepts no arguments")
		}
		program := tea.NewProgram(
			interactiveModel{},
			tea.WithAltScreen(),
			tea.WithMouseCellMotion(),
			tea.WithInput(os.Stdin),
			tea.WithOutput(stdout),
		)
		_, err := program.Run()
		return err
	case "help", "-h", "--help":
		return usage(stdout)
	default:
		return fmt.Errorf("unknown fixture %q", arguments[0])
	}
}

func usage(output io.Writer) error {
	_, err := fmt.Fprintln(output, `usage: go run ./tools/terminal-acceptance <fixture>

fixtures:
  inventory             print platform and executable availability only
  render                print ANSI, Unicode, emoji, CJK, and OSC 8 fixtures
  output --mib 100      stream deterministic bounded high-volume output
  interactive           exercise alternate screen, resize, keys, IME, and mouse`)
	return err
}

func writeInventory(
	output io.Writer,
	lookPath func(string) (string, error),
	getenv func(string) string,
) error {
	programs := append([]string(nil), inventoryPrograms...)
	sort.Strings(programs)
	if _, err := fmt.Fprintf(
		output,
		"os=%s arch=%s term=%s utf8_locale=%s\n",
		runtime.GOOS,
		runtime.GOARCH,
		presence(getenv("TERM")),
		presence(firstNonempty(getenv("LC_ALL"), getenv("LC_CTYPE"), getenv("LANG"))),
	); err != nil {
		return err
	}
	for _, program := range programs {
		status := "missing"
		if _, err := lookPath(program); err == nil {
			status = "available"
		}
		if _, err := fmt.Fprintf(output, "%s=%s\n", program, status); err != nil {
			return err
		}
	}
	return nil
}

func firstNonempty(values ...string) string {
	for _, value := range values {
		if value != "" {
			return value
		}
	}
	return ""
}

func presence(value string) string {
	if strings.TrimSpace(value) == "" {
		return "unset"
	}
	return "set"
}

func writeRenderFixture(output io.Writer) error {
	const fixture = "" +
		"p-track terminal render fixture\r\n" +
		"ANSI: \x1b[31mred\x1b[0m \x1b[32mgreen\x1b[0m \x1b[38;2;61;214;163mtruecolor\x1b[0m\r\n" +
		"Unicode: cafe\u0301 | caf\u00e9 | \u03bb\u03a9 | \u0416\u042f\r\n" +
		"Wide: \u65e5\u672c\u8a9e | \ud55c\uad6d\uc5b4 | \u4e2d\u6587\r\n" +
		"Emoji: \U0001f680 \U0001f9d1\u200d\U0001f4bb \U0001f3f3\ufe0f\u200d\U0001f308 \U0001f1fa\U0001f1f3\r\n" +
		"Hyperlink: \x1b]8;;https://example.com/ptrack-terminal-acceptance\x1b\\example.com fixture\x1b]8;;\x1b\\\r\n" +
		"Expected: aligned separators, one cell-width model, and modifier-click-only link opening.\r\n"
	_, err := io.WriteString(output, fixture)
	return err
}

func writeOutputFixture(output io.Writer, mib int) error {
	if mib < 1 || mib > maximumOutputMiB {
		return fmt.Errorf("--mib must be between 1 and %d", maximumOutputMiB)
	}
	line := []byte("0123456789abcdef p-track bounded output fixture\r\n")
	block := bytes.Repeat(line, outputBlockBytes/len(line)+1)
	block = block[:outputBlockBytes]
	remaining := int64(mib) * 1024 * 1024
	for remaining > 0 {
		chunk := int64(len(block))
		if chunk > remaining {
			chunk = remaining
		}
		written, err := output.Write(block[:chunk])
		if err != nil {
			return err
		}
		if written == 0 {
			return io.ErrShortWrite
		}
		remaining -= int64(written)
	}
	return nil
}

type interactiveModel struct {
	width      int
	height     int
	runes      int
	composing  bool
	mouseEvent string
}

func (interactiveModel) Init() tea.Cmd { return nil }

func (model interactiveModel) Update(message tea.Msg) (tea.Model, tea.Cmd) {
	switch message := message.(type) {
	case tea.WindowSizeMsg:
		model.width = message.Width
		model.height = message.Height
	case tea.KeyMsg:
		switch message.String() {
		case "ctrl+c", "ctrl+d", "esc", "q":
			return model, tea.Quit
		}
		if message.Type == tea.KeyRunes {
			model.runes += utf8.RuneCountInString(string(message.Runes))
			model.composing = message.Paste
		}
	case tea.MouseMsg:
		model.mouseEvent = fmt.Sprintf("%v at %d,%d", message.Action, message.X, message.Y)
	}
	return model, nil
}

func (model interactiveModel) View() string {
	mouse := model.mouseEvent
	if mouse == "" {
		mouse = "none"
	}
	return strings.Join([]string{
		"p-track interactive terminal fixture",
		"",
		"Resize the window or pane; the dimensions below must follow it.",
		"Type with the active keyboard layout and IME; only rune counts are shown.",
		"Move, click, and scroll the mouse; the last event must update.",
		"Verify this alternate-screen view disappears cleanly on exit.",
		"",
		"size=" + strconv.Itoa(model.width) + "x" + strconv.Itoa(model.height),
		"typed_runes=" + strconv.Itoa(model.runes),
		"paste_or_composition_event=" + strconv.FormatBool(model.composing),
		"mouse=" + mouse,
		"",
		"Exit with Esc, q, Ctrl+C, or Ctrl+D.",
	}, "\r\n")
}
