package tui

import (
	"fmt"
	"strings"

	"github.com/charmbracelet/lipgloss"
	"github.com/charmbracelet/x/ansi"
	"github.com/ro-ag/ptrack/internal/model"
)

// Palette.
var (
	// A cool-neutral base keeps the expanded btop-like spectrum cohesive.
	cMagenta  = lipgloss.Color("#FF5FD7")
	cIndigo   = lipgloss.Color("#6D5CFF")
	cLavender = lipgloss.Color("#AFA8FF")
	cBlue     = lipgloss.Color("#5FAFFF")
	cCyan     = lipgloss.Color("#2AA7A1")
	cTeal     = lipgloss.Color("#3DD6A3")
	cGreen    = lipgloss.Color("#5FFF87")
	cAmber    = lipgloss.Color("#FFD75F")
	cRed      = lipgloss.Color("#FF5F87")
	cText     = lipgloss.Color("#E6E9F0")
	cGray     = lipgloss.Color("#B7C0D8")
	cDim      = lipgloss.Color("#727A8E")
	cFaint    = lipgloss.Color("#313244")
	cBorder   = lipgloss.Color("#45475A")
)

type rgb struct{ r, g, b uint8 }

var (
	gradDarkCyan  = rgb{0x17, 0x8F, 0x95}
	gradBlueGreen = rgb{0x3C, 0xD1, 0xA5}
	gradMagenta   = rgb{0xFF, 0x5F, 0xD7}
	gradIndigo    = rgb{0x6D, 0x5C, 0xFF}
)

var (
	labelStyle  = lipgloss.NewStyle().Bold(true).Foreground(cCyan)
	dimStyle    = lipgloss.NewStyle().Foreground(cDim)
	textStyle   = lipgloss.NewStyle().Foreground(cText)
	statusStyle = lipgloss.NewStyle().Foreground(cAmber)
	cursorStyle = lipgloss.NewStyle().Foreground(cMagenta).Bold(true)
	activeStyle = lipgloss.NewStyle().Foreground(cGreen).Bold(true)
	keyStyle    = lipgloss.NewStyle().Foreground(cLavender).Bold(true)
	hintStyle   = lipgloss.NewStyle().Foreground(cGray)

	tabActiveStyle   = lipgloss.NewStyle().Bold(true).Foreground(cText).Background(cCyan).Padding(0, 1)
	tabInactiveStyle = lipgloss.NewStyle().Foreground(cGray).Padding(0, 1)

	selRowStyle = lipgloss.NewStyle().Foreground(lipgloss.Color("231")).Background(cFaint).Bold(true)
)

// fitLine renders exactly width cells, safely truncating ANSI-styled content.
func fitLine(s string, width int) string {
	if width < 1 {
		return ""
	}
	return lipgloss.NewStyle().Width(width).Render(ansi.Truncate(s, width, ""))
}

// selectedLine gives the active row a full-width, quiet selection surface.
func selectedLine(s string, width int) string {
	if width < 1 {
		return ""
	}
	return selRowStyle.Width(width).Render(ansi.Truncate(s, width, ""))
}

// gradientText applies a restrained multi-stop foreground gradient. It is used
// only for decorative chrome; semantic status colors remain discrete.
func gradientText(s string, stops ...rgb) string {
	runes := []rune(s)
	if len(runes) == 0 || len(stops) == 0 {
		return s
	}
	if len(stops) == 1 || len(runes) == 1 {
		c := stops[0]
		return lipgloss.NewStyle().Foreground(lipgloss.Color(fmt.Sprintf("#%02X%02X%02X", c.r, c.g, c.b))).Render(s)
	}

	var out strings.Builder
	for i, r := range runes {
		scaled := float64(i) * float64(len(stops)-1) / float64(len(runes)-1)
		segment := int(scaled)
		mix := scaled - float64(segment)
		if segment >= len(stops)-1 {
			segment = len(stops) - 2
			mix = 1
		}
		from, to := stops[segment], stops[segment+1]
		lerp := func(a, b uint8) uint8 { return uint8(float64(a) + (float64(b)-float64(a))*mix) }
		c := rgb{lerp(from.r, to.r), lerp(from.g, to.g), lerp(from.b, to.b)}
		out.WriteString(lipgloss.NewStyle().Foreground(lipgloss.Color(fmt.Sprintf("#%02X%02X%02X", c.r, c.g, c.b))).Render(string(r)))
	}
	return out.String()
}

// panelContentWidth is the usable row width inside panel borders and padding.
func panelContentWidth(width int) int {
	return max(1, width-4)
}

func taskStatusColor(s model.TaskStatus) lipgloss.Color {
	switch s {
	case model.TaskDone:
		return cGreen
	case model.TaskDoing:
		return cAmber
	case model.TaskBlocked:
		return cRed
	default:
		return cLavender
	}
}

func taskIcon(s model.TaskStatus) string {
	switch s {
	case model.TaskDone:
		return "✓"
	case model.TaskDoing:
		return "◐"
	case model.TaskBlocked:
		return "✗"
	default:
		return "○"
	}
}

func severityColor(s model.Severity) lipgloss.Color {
	switch s {
	case model.SeverityCritical:
		return cRed
	case model.SeverityHigh:
		return cAmber
	case model.SeverityMedium:
		return cBlue
	default:
		return cTeal
	}
}

func planStatusColor(s model.PlanStatus) lipgloss.Color {
	switch s {
	case model.PlanDone:
		return cGreen
	case model.PlanArchived:
		return cDim
	default:
		return cBlue
	}
}

// truncate shortens s to n runes with an ellipsis when cut.
func truncate(s string, n int) string {
	if n < 1 {
		n = 1
	}
	r := []rune(s)
	if len(r) <= n {
		return s
	}
	if n <= 1 {
		return string(r[:n])
	}
	return string(r[:n-1]) + "…"
}

// windowRange returns the [start,end) slice of n items to show for height rows,
// keeping cursor visible.
func windowRange(n, cursor, height int) (int, int) {
	if height <= 0 || n <= height {
		return 0, n
	}
	start := cursor - height/2
	if start < 0 {
		start = 0
	}
	if start+height > n {
		start = n - height
	}
	if start < 0 {
		start = 0
	}
	return start, start + height
}

func clamp(v, lo, hi int) int {
	if hi < lo {
		return lo
	}
	if v < lo {
		return lo
	}
	if v > hi {
		return hi
	}
	return v
}
