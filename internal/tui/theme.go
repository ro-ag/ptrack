package tui

import (
	"fmt"
	"strings"

	"github.com/charmbracelet/lipgloss"
	"github.com/charmbracelet/x/ansi"
	"github.com/ro-ag/ptrack/internal/model"
)

// Palette. Chrome — borders, tabs, keys, brand, focus — lives entirely in the
// teal accent family plus neutral grays. The remaining hues are data colors,
// reserved for task states, plan states, and issue severities, so meaning is
// the only thing that gets color.
var (
	cAccent    = lipgloss.Color("#3DD6A3") // focused chrome, brand, keys
	cAccentDim = lipgloss.Color("#2AA7A1") // secondary chrome (section titles, groups)
	cInk       = lipgloss.Color("#081316") // text on accent-filled surfaces
	cLavender  = lipgloss.Color("#AFA8FF") // todo tasks, open milestones
	cBlue      = lipgloss.Color("#5FAFFF") // active plans, medium severity
	cGreen     = lipgloss.Color("#5FFF87") // done
	cAmber     = lipgloss.Color("#FFD75F") // doing, high severity, status toast
	cRed       = lipgloss.Color("#FF5F87") // blocked, critical, open issues
	cText      = lipgloss.Color("#E6E9F0")
	cGray      = lipgloss.Color("#B7C0D8")
	cDim       = lipgloss.Color("#727A8E")
	cFaint     = lipgloss.Color("#313244")
	cBorder    = lipgloss.Color("#45475A")
)

// The house gradient runs deep-cyan → blue-green. It is the visual signature
// of the app and appears ONLY on decorative chrome — border lines, rules, the
// wordmark — never on words: gradient text is hard to read, so titles and
// labels always render in a single solid color.
type rgb struct{ r, g, b uint8 }

var (
	gradDarkCyan  = rgb{0x17, 0x8F, 0x95}
	gradBlueGreen = rgb{0x3C, 0xD1, 0xA5}
	gradAccent    = rgb{0x3D, 0xD6, 0xA3} // cAccent, for band fades
	gradNight     = rgb{0x0C, 0x10, 0x16} // near-terminal-black fade target
)

var (
	labelStyle  = lipgloss.NewStyle().Bold(true).Foreground(cAccent)
	dimStyle    = lipgloss.NewStyle().Foreground(cDim)
	textStyle   = lipgloss.NewStyle().Foreground(cText)
	statusStyle = lipgloss.NewStyle().Foreground(cAmber)
	activeStyle = lipgloss.NewStyle().Foreground(cGreen).Bold(true)
	keyStyle    = lipgloss.NewStyle().Foreground(cAccent).Bold(true)
	hintStyle   = lipgloss.NewStyle().Foreground(cGray)
	brandStyle  = lipgloss.NewStyle().Bold(true).Foreground(cInk).Background(cAccent).Padding(0, 1)
	groupStyle  = lipgloss.NewStyle().Bold(true).Foreground(cAccentDim)

	tabActiveStyle = lipgloss.NewStyle().Bold(true).Foreground(cInk).Background(cAccent).Padding(0, 1)

	borderStyle      = lipgloss.NewStyle().Foreground(cBorder)
	focusBorderStyle = lipgloss.NewStyle().Foreground(cAccent)

	selBarStyle = lipgloss.NewStyle().Foreground(cAccent).Background(cFaint)
	selRowStyle = lipgloss.NewStyle().Foreground(lipgloss.Color("231")).Background(cFaint).Bold(true)
)

// fitLine renders exactly width cells, safely truncating ANSI-styled content.
func fitLine(s string, width int) string {
	if width < 1 {
		return ""
	}
	return lipgloss.NewStyle().Width(width).Render(ansi.Truncate(s, width, ""))
}

// selectedLine gives the active row a full-width selection surface with an
// accent bar on its left edge, btop-style.
func selectedLine(s string, width int) string {
	if width < 3 {
		if width < 1 {
			return ""
		}
		return selRowStyle.Width(width).Render(ansi.Truncate(s, width, ""))
	}
	bar := selBarStyle.Render("▌")
	body := selRowStyle.Width(width - 1).Render(" " + ansi.Truncate(s, width-2, ""))
	return bar + body
}

// gradientText applies a restrained multi-stop foreground gradient. Chrome
// only (rules, borders, the wordmark) — readable text stays solid.
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

// bgFade renders a run of blank cells whose background sweeps from one color
// to another, letting an accent band melt into the terminal background.
func bgFade(width int, from, to rgb) string {
	if width < 1 {
		return ""
	}
	var out strings.Builder
	for i := 0; i < width; i++ {
		t := float64(i) / float64(max(1, width-1))
		lerp := func(a, b uint8) uint8 { return uint8(float64(a) + (float64(b)-float64(a))*t) }
		c := rgb{lerp(from.r, to.r), lerp(from.g, to.g), lerp(from.b, to.b)}
		out.WriteString(lipgloss.NewStyle().Background(lipgloss.Color(fmt.Sprintf("#%02X%02X%02X", c.r, c.g, c.b))).Render(" "))
	}
	return out.String()
}

// meter renders a compact done/total progress bar; the filled portion carries
// the data color, the empty portion stays in border gray.
func meter(done, total, width int, fill lipgloss.Color) string {
	if width < 1 {
		return ""
	}
	filled := 0
	if total > 0 {
		filled = done * width / total
		if done > 0 && filled == 0 {
			filled = 1
		}
		if filled > width {
			filled = width
		}
	}
	on := lipgloss.NewStyle().Foreground(fill).Render(strings.Repeat("▰", filled))
	off := lipgloss.NewStyle().Foreground(cBorder).Render(strings.Repeat("▱", width-filled))
	return on + off
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
