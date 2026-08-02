package gui

import (
	"time"

	"github.com/ro-ag/ptrack/internal/model"
)

const (
	heatmapDefaultWeeks = 16
	heatmapMaxWeeks     = 52
)

// HeatmapDay is one zero-filled day of note+commit activity.
type HeatmapDay struct {
	Date  string `json:"date"` // YYYY-MM-DD, local time
	Count int    `json:"count"`
}

// GetActivityHeatmapV2 returns a dense, zero-filled series of daily
// note+commit counts for the last `weeks` weeks (inclusive of today),
// oldest first. weeks<=0 defaults to 16; clamp to <=52.
func (a *App) GetActivityHeatmapV2(weeks int) ([]HeatmapDay, error) {
	if weeks <= 0 {
		weeks = heatmapDefaultWeeks
	}
	if weeks > heatmapMaxWeeks {
		weeks = heatmapMaxWeeks
	}
	s, _, release, err := a.openWorkspace(0)
	if err != nil {
		return nil, err
	}
	defer release()
	defer s.Close()

	notes, err := s.ListNotes()
	if err != nil {
		return nil, err
	}
	commits, err := s.ListCommits()
	if err != nil {
		return nil, err
	}
	return buildHeatmap(weeks*7, time.Now(), notes, commits), nil
}

// buildHeatmap buckets note and commit CreatedAt values by local calendar
// day into a dense series of `days` entries ending today, oldest first.
func buildHeatmap(
	days int,
	now time.Time,
	notes []model.Note,
	commits []model.Commit,
) []HeatmapDay {
	today := time.Date(now.Year(), now.Month(), now.Day(), 0, 0, 0, 0, time.Local)
	start := today.AddDate(0, 0, -(days - 1))
	counts := make(map[string]int, days)
	bucket := func(at time.Time) {
		local := at.Local()
		day := time.Date(local.Year(), local.Month(), local.Day(), 0, 0, 0, 0, time.Local)
		if day.Before(start) || day.After(today) {
			return
		}
		counts[day.Format("2006-01-02")]++
	}
	for _, note := range notes {
		bucket(note.CreatedAt)
	}
	for _, commit := range commits {
		bucket(commit.CreatedAt)
	}
	series := make([]HeatmapDay, 0, days)
	for offset := 0; offset < days; offset++ {
		day := start.AddDate(0, 0, offset)
		key := day.Format("2006-01-02")
		series = append(series, HeatmapDay{Date: key, Count: counts[key]})
	}
	return series
}
