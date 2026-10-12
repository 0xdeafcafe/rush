package gate

import (
	"os"
	"runtime"
	"strconv"
	"strings"
)

// WorkerEnv is what a gated vitest run as name is told of its share of the
// machine: a third of the cores, at least one, so runs side by side don't
// pile workers on them and its memory stays near its estimate. A count it
// was given stands.
func WorkerEnv(name string, argv []string) []string {
	if name != "vitest" || os.Getenv("VITEST_MAX_WORKERS") != "" {
		return nil
	}
	line := strings.Join(argv, " ")
	for _, f := range []string{"--maxWorkers", "--max-workers", "--pool.maxWorkers", "VITEST_MAX_WORKERS", "--no-file-parallelism"} {
		if strings.Contains(line, f) {
			return nil
		}
	}
	return []string{"VITEST_MAX_WORKERS=" + strconv.Itoa(max(1, runtime.NumCPU()/3))}
}
