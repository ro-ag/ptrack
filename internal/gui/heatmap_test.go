package gui

import (
	"errors"
	"testing"
	"time"

	"github.com/ro-ag/ptrack/internal/model"
)

func TestBuildHeatmapReturnsDenseZeroFilledSeries(t *testing.T) {
	now := time.Date(2026, 8, 2, 15, 30, 0, 0, time.Local)
	series := buildHeatmap(14, now, nil, nil)
	if len(series) != 14 {
		t.Fatalf("series length = %d, want 14", len(series))
	}
	if series[0].Date != "2026-07-20" || series[13].Date != "2026-08-02" {
		t.Fatalf("series bounds = %q..%q, want 2026-07-20..2026-08-02",
			series[0].Date, series[13].Date)
	}
	for index, day := range series {
		if day.Count != 0 {
			t.Fatalf("series[%d] = %#v, want zero-filled", index, day)
		}
		previous := series[max(0, index-1)].Date
		if index > 0 && day.Date <= previous {
			t.Fatalf("series not strictly increasing at %d: %q after %q", index, day.Date, previous)
		}
	}
}

func TestBuildHeatmapBucketsNotesAndCommitsByLocalDay(t *testing.T) {
	now := time.Date(2026, 8, 2, 15, 30, 0, 0, time.Local)
	yesterday := now.AddDate(0, 0, -1)
	lastWeek := now.AddDate(0, 0, -6)
	tooOld := now.AddDate(0, 0, -7)
	notes := []model.Note{
		{ID: 1, CreatedAt: now},
		{ID: 2, CreatedAt: now.Add(-time.Hour)},
		{ID: 3, CreatedAt: lastWeek},
		{ID: 4, CreatedAt: tooOld},
	}
	commits := []model.Commit{
		{ID: 1, CreatedAt: yesterday},
		{ID: 2, CreatedAt: lastWeek},
	}
	series := buildHeatmap(7, now, notes, commits)
	if len(series) != 7 {
		t.Fatalf("series length = %d, want 7", len(series))
	}
	counts := make(map[string]int, len(series))
	for _, day := range series {
		counts[day.Date] = day.Count
	}
	want := map[string]int{
		now.Format("2006-01-02"):       2,
		yesterday.Format("2006-01-02"): 1,
		lastWeek.Format("2006-01-02"):  2,
	}
	for date, count := range want {
		if counts[date] != count {
			t.Fatalf("count for %s = %d, want %d (series %#v)", date, counts[date], count, series)
		}
	}
	total := 0
	for _, day := range series {
		total += day.Count
	}
	if total != 5 {
		t.Fatalf("total = %d, want 5 (out-of-range note excluded)", total)
	}
}

func TestGetActivityHeatmapV2DefaultsAndClampsWeeks(t *testing.T) {
	app := seedApp(t)

	series, err := app.GetActivityHeatmapV2(0)
	if err != nil {
		t.Fatalf("GetActivityHeatmapV2: %v", err)
	}
	if len(series) != heatmapDefaultWeeks*7 {
		t.Fatalf("default length = %d, want %d", len(series), heatmapDefaultWeeks*7)
	}
	today := time.Now().Format("2006-01-02")
	if series[len(series)-1].Date != today {
		t.Fatalf("last day = %q, want today %q", series[len(series)-1].Date, today)
	}
	// The seeded note and commit were both created today.
	if series[len(series)-1].Count != 2 {
		t.Fatalf("today count = %d, want 2", series[len(series)-1].Count)
	}

	clamped, err := app.GetActivityHeatmapV2(500)
	if err != nil {
		t.Fatalf("GetActivityHeatmapV2 clamp: %v", err)
	}
	if len(clamped) != heatmapMaxWeeks*7 {
		t.Fatalf("clamped length = %d, want %d", len(clamped), heatmapMaxWeeks*7)
	}
}

func TestGetActivityHeatmapV2RequiresOpenWorkspace(t *testing.T) {
	app := newWorkspaceCoordinator(nil, nil)
	if _, err := app.GetActivityHeatmapV2(4); !errors.Is(err, errNoWorkspace) {
		t.Fatalf("GetActivityHeatmapV2 without project = %v, want errNoWorkspace", err)
	}
}
