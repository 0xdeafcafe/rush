package gate

import (
	"testing"

	"github.com/0xdeafcafe/rush/internal/proc"
)

func stubLevels(t *testing.T, kernel, mem int) {
	oldP, oldM := PressureLevel, MemoryLevel
	PressureLevel = func() int { return kernel }
	MemoryLevel = func() int { return mem }
	t.Cleanup(func() { PressureLevel, MemoryLevel = oldP, oldM })
}

// macOS can say normal while the machine pages itself to a halt: memory
// at the floor is critical whatever the kernel says, near it a warning.
func TestPressureFromMemoryLeft(t *testing.T) {
	for _, c := range []struct{ kernel, mem, floor, want int }{
		{PressureNormal, 60, 5, PressureNormal},
		{PressureNormal, 4, 5, PressureCritical},
		{PressureNormal, 5, 0, PressureCritical},
		{PressureNormal, 9, 5, PressureWarn},
		{PressureCritical, 60, 5, PressureCritical},
		{PressureNormal, -1, 5, PressureNormal},
		{0, -1, 5, 0},
	} {
		stubLevels(t, c.kernel, c.mem)
		if got := Pressure(c.floor); got != c.want {
			t.Errorf("Pressure(%d) with kernel %d, memory %d%% = %d, want %d", c.floor, c.kernel, c.mem, got, c.want)
		}
	}
}

// table is a process table from pid, ppid, comm and footprint in MiB.
func table(rows ...[4]any) *proc.Table {
	t := &proc.Table{Procs: map[int]*proc.Proc{}, Children: map[int][]int{}}
	for _, r := range rows {
		p := &proc.Proc{PID: r[0].(int), PPID: r[1].(int), Comm: r[2].(string), Footprint: uint64(r[3].(int)) << 20}
		t.Procs[p.PID] = p
		t.Children[p.PPID] = append(t.Children[p.PPID], p.PID)
	}
	return t
}

// As the panic of 2026-10-23 had it: three tsgo language servers under
// agents, none through the gate, beside a browser that isn't an agent's.
func TestChooseTheLargestOfAgents(t *testing.T) {
	tab := table(
		[4]any{100, 1, "rush", 80},       // the view
		[4]any{101, 100, "rush", 40},     // the plugin broker
		[4]any{102, 101, "rush", 30},     // a plugin
		[4]any{110, 100, "rush", 20},     // a session's host
		[4]any{111, 110, "claude", 4000}, // its agent: never stopped
		[4]any{112, 111, "node", 300},    // an MCP server
		[4]any{113, 112, "tsgo", 9700},   // the language server it started
		[4]any{120, 100, "rush", 20},     // another host
		[4]any{121, 120, "claude", 500},  // its agent
		[4]any{122, 121, "bash", 5},      // a Bash call
		[4]any{123, 122, "rush", 9999},   // the gate's holder: rush's own
		[4]any{124, 123, "tsgo", 4100},   // the run it holds
		[4]any{200, 1, "Arc", 20000},     // not an agent's
		[4]any{201, 200, "Browser Helper", 12000},
	)
	agents := Agents(tab)
	if len(agents) != 2 {
		t.Fatalf("Agents = %v, want the two claude", agents)
	}
	v, ok := Choose(tab, agents, 0)
	if !ok || v.PID != 113 || v.Agent != 111 {
		t.Fatalf("Choose = %+v, %v; want tsgo 113 under agent 111", v, ok)
	}
	if v, ok := Choose(tab, agents, 113); !ok || v.PID != 124 {
		t.Fatalf("Choose leaving out itself = %+v, %v; want tsgo 124", v, ok)
	}
}

// When what fills the memory isn't an agent's, the guard stops nothing.
func TestChooseNothingSmall(t *testing.T) {
	tab := table(
		[4]any{100, 1, "rush", 80},
		[4]any{110, 100, "rush", 20},
		[4]any{111, 110, "claude", 6000},
		[4]any{112, 111, "bash", 100},
	)
	if v, ok := Choose(tab, Agents(tab), 0); ok {
		t.Fatalf("Choose = %+v, want none", v)
	}
}

func TestParseFloor(t *testing.T) {
	for in, want := range map[string]int{"5%": 5, "10%": 10, "off": 0, "nonsense": DefaultFloor, "80%": DefaultFloor} {
		if got := ParseFloor(map[string]string{"floor": in}); got != want {
			t.Errorf("ParseFloor(%q) = %d, want %d", in, got, want)
		}
	}
	if got := ParseFloor(nil); got != DefaultFloor {
		t.Errorf("ParseFloor(nil) = %d", got)
	}
}
