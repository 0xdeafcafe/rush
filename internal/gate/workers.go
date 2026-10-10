package gate

import (
	"os"
	"runtime"
	"strconv"
	"strings"
)

// WorkerEnv is what a gated vitest run as name is told of its share of the
// machine: the cores idle now, at most the cores over Parallel, at least
// one, so a full queue keeps every core busy without piling workers on
// them. A count it was given stands.
func WorkerEnv(r Rule, name string, argv []string) []string {
	if name != "vitest" || os.Getenv("VITEST_MAX_WORKERS") != "" {
		return nil
	}
	line := strings.Join(argv, " ")
	for _, f := range []string{"--maxWorkers", "--max-workers", "--pool.maxWorkers", "VITEST_MAX_WORKERS", "--no-file-parallelism"} {
		if strings.Contains(line, f) {
			return nil
		}
	}
	cpus := runtime.NumCPU()
	n := cpus / max(r.Parallel, 1)
	if l := load1(); l >= 0 {
		n = min(n, cpus-int(l+0.5))
	}
	return []string{"VITEST_MAX_WORKERS=" + strconv.Itoa(max(1, n))}
}
