package tui

import (
	"github.com/charmbracelet/lipgloss"
	"github.com/ro-ag/ptrack/internal/model"
)

// Palette.
var (
	cPink   = lipgloss.Color("212")
	cTitle  = lipgloss.Color("205")
	cBlue   = lipgloss.Color("39")
	cDim    = lipgloss.Color("241")
	cFaint  = lipgloss.Color("237")
	cGreen  = lipgloss.Color("42")
	cAmber  = lipgloss.Color("214")
	cRed    = lipgloss.Color("203")
	cGray   = lipgloss.Color("245")
	cText   = lipgloss.Color("252")
	cBorder = lipgloss.Color("240")
)

var (
	appTitleStyle = lipgloss.NewStyle().Bold(true).Foreground(cTitle)
	labelStyle    = lipgloss.NewStyle().Bold(true).Foreground(cBlue)
	dimStyle      = lipgloss.NewStyle().Foreground(cDim)
	textStyle     = lipgloss.NewStyle().Foreground(cText)
	statusStyle   = lipgloss.NewStyle().Foreground(cAmber)
	cursorStyle   = lipgloss.NewStyle().Foreground(cPink).Bold(true)
	activeStyle   = lipgloss.NewStyle().Foreground(cGreen).Bold(true)

	tabActiveStyle   = lipgloss.NewStyle().Bold(true).Foreground(lipgloss.Color("231")).Background(cPink).Padding(0, 2)
	tabInactiveStyle = lipgloss.NewStyle().Foreground(cDim).Padding(0, 2)

	selRowStyle = lipgloss.NewStyle().Foreground(lipgloss.Color("231")).Background(cFaint).Bold(true)
)

// panelStyle returns a bordered panel box of the given size, brighter when focused.
func panelStyle(width, height int, focused bool) lipgloss.Style {
	border := cBorder
	if focused {
		border = cPink
	}
	s := lipgloss.NewStyle().
		Border(lipgloss.RoundedBorder()).
		BorderForeground(border).
		Padding(0, 1)
	if width > 0 {
		s = s.Width(width)
	}
	if height > 0 {
		s = s.Height(height)
	}
	return s
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
		return cGray
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
		return cGray
	}
}

func planStatusColor(s model.PlanStatus) lipgloss.Color {
	switch s {
	case model.PlanDone:
		return cGreen
	case model.PlanArchived:
		return cDim
	default:
		return cText
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
