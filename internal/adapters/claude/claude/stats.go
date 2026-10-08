package claude

import (
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent"
)

// Stats is Claude Code's own record of an account's use over time, as it
// keeps it for /stats in stats-cache.json.
type (
	Stats      = agent.Stats
	StatsDay   = agent.StatsDay
	ModelTotal = agent.ModelTotal
)

// LoadStats reads the account's stats-cache.json.
func LoadStats(acct Account) (Stats, error) {
	b, err := os.ReadFile(filepath.Join(acct.ConfigDir, "stats-cache.json"))
	if err != nil {
		return Stats{}, err
	}
	var raw struct {
		LastComputedDate string `json:"lastComputedDate"`
		DailyActivity    []struct {
			Date         string `json:"date"`
			MessageCount int    `json:"messageCount"`
			SessionCount int    `json:"sessionCount"`
			ToolCalls    int    `json:"toolCallCount"`
		} `json:"dailyActivity"`
		DailyModelTokens []struct {
			Date   string           `json:"date"`
			Tokens map[string]int64 `json:"tokensByModel"`
		} `json:"dailyModelTokens"`
		ModelUsage map[string]struct {
			In         int64   `json:"inputTokens"`
			Out        int64   `json:"outputTokens"`
			CacheRead  int64   `json:"cacheReadInputTokens"`
			CacheWrite int64   `json:"cacheCreationInputTokens"`
			CostUSD    float64 `json:"costUSD"`
		} `json:"modelUsage"`
		TotalSessions  int    `json:"totalSessions"`
		TotalMessages  int    `json:"totalMessages"`
		FirstSession   string `json:"firstSessionDate"`
		LongestSession struct {
			SessionID    string `json:"sessionId"`
			Timestamp    string `json:"timestamp"`
			Duration     int64  `json:"duration"`
			MessageCount int    `json:"messageCount"`
		} `json:"longestSession"`
		HourCounts map[string]int `json:"hourCounts"`
	}
	if err := jsonx.Unmarshal(b, &raw); err != nil {
		return Stats{}, err
	}
	s := Stats{Computed: raw.LastComputedDate, Sessions: raw.TotalSessions, Messages: raw.TotalMessages,
		Longest: time.Duration(raw.LongestSession.Duration) * time.Millisecond, LongestMsgs: raw.LongestSession.MessageCount,
		LongestID: raw.LongestSession.SessionID}
	s.First, _ = time.Parse(time.RFC3339, raw.FirstSession)
	s.LongestAt, _ = time.Parse(time.RFC3339, raw.LongestSession.Timestamp)
	days := map[string]*StatsDay{}
	day := func(d string) *StatsDay {
		if days[d] == nil {
			t, _ := time.ParseInLocation("2006-01-02", d, time.Local)
			days[d] = &StatsDay{Date: t}
		}
		return days[d]
	}
	for _, a := range raw.DailyActivity {
		d := day(a.Date)
		d.Messages, d.Sessions, d.ToolCalls = a.MessageCount, a.SessionCount, a.ToolCalls
	}
	for _, t := range raw.DailyModelTokens {
		day(t.Date).Tokens = t.Tokens
	}
	for _, d := range days {
		s.Days = append(s.Days, *d)
	}
	sort.Slice(s.Days, func(i, j int) bool { return s.Days[i].Date.Before(s.Days[j].Date) })
	for name, u := range raw.ModelUsage {
		s.Models = append(s.Models, ModelTotal{Model: name, In: u.In, Out: u.Out, CacheRead: u.CacheRead, CacheWrite: u.CacheWrite, CostUSD: u.CostUSD})
	}
	sort.Slice(s.Models, func(i, j int) bool { return s.Models[i].Total() > s.Models[j].Total() })
	for h, n := range raw.HourCounts {
		if i, err := strconv.Atoi(h); err == nil && i >= 0 && i < 24 {
			s.Hours[i] = n
		}
	}
	return s, nil
}
