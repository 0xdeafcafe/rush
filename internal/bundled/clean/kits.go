package clean

import (
	"context"
	"os"
	"path/filepath"
	"slices"
	"strconv"
	"strings"

	"github.com/0xdeafcafe/rush/internal/fleet"
	"github.com/0xdeafcafe/rush/internal/ghpr"
)

var goKit = &kit{
	name: "go-clean",
	about: "Go's build and module caches, gopls's and golangci-lint's, and the gopls processes agents' worktrees leave running: " +
		"how much each takes, and cleaning it up when you pick it. Off until you turn it on; needs go.",
	needs: []string{"go"},
	procs: []string{"gopls", "golangci-lint", "dlv"},
	things: func(ctx context.Context, _ []session) []thing {
		var out []thing
		if d, err := command(ctx, "", "go", "env", "GOCACHE"); err == nil && isDir(d) {
			out = append(out, thing{ID: "gocache", Title: "Go build cache", Path: d,
				Does: "runs go clean -cache: builds and tests are slower until it fills again", Clean: ran("", "go", "clean", "-cache")})
		}
		if d, err := command(ctx, "", "go", "env", "GOMODCACHE"); err == nil && isDir(d) {
			out = append(out, thing{ID: "gomodcache", Title: "Go module cache", Path: d,
				Does: "runs go clean -modcache: every module downloads again when next built", Clean: ran("", "go", "clean", "-modcache")})
		}
		for _, c := range []struct{ id, title, dir string }{
			{"gopls", "gopls cache", "gopls"},
			{"golangci-lint", "golangci-lint cache", "golangci-lint"},
		} {
			if d := cacheDir(c.dir); d != "" && isDir(d) {
				out = append(out, thing{ID: c.id, Title: c.title, Path: d, Does: "deletes it; it's built again as needed", Clean: removed(d)})
			}
		}
		return out
	},
}

var jsKit = &kit{
	name: "js-clean",
	about: "pnpm's store, npm's, yarn's and bun's caches, node_modules in agents' worktrees, and the node processes left running: " +
		"how much each takes, and cleaning it up when you pick it. Off until you turn it on; needs node.",
	needs: []string{"node"},
	procs: []string{"node", "bun", "deno"},
	things: func(ctx context.Context, seen []session) []thing {
		var out []thing
		if d, err := command(ctx, "", "pnpm", "store", "path"); err == nil && isDir(d) {
			out = append(out, thing{ID: "pnpm", Title: "pnpm store", Path: d,
				Does: "runs pnpm store prune: removes packages no project uses", Clean: ran("", "pnpm", "store", "prune")})
		}
		if d, err := command(ctx, "", "npm", "config", "get", "cache"); err == nil && isDir(d) {
			out = append(out, thing{ID: "npm", Title: "npm cache", Path: d,
				Does: "runs npm cache clean --force: packages download again when next installed", Clean: ran("", "npm", "cache", "clean", "--force")})
		}
		if d, err := command(ctx, "", "yarn", "cache", "dir"); err == nil && isDir(d) {
			out = append(out, thing{ID: "yarn", Title: "yarn cache", Path: d,
				Does: "runs yarn cache clean: packages download again when next installed", Clean: ran("", "yarn", "cache", "clean")})
		}
		if d := bunCache(); isDir(d) {
			out = append(out, thing{ID: "bun", Title: "bun cache", Path: d,
				Does: "runs bun pm cache rm: packages download again when next installed", Clean: ran("", "bun", "pm", "cache", "rm")})
		}
		// Dependencies in worktrees no agent is working in. The main
		// checkout's are left alone: that's where you work.
		wts, busy := worktrees(seen)
		for _, w := range wts {
			nm := filepath.Join(w.Path, "node_modules")
			if busy[w.Path] || !isDir(nm) {
				continue
			}
			out = append(out, thing{ID: "nm:" + w.Path, Title: "node_modules in " + filepath.Base(w.Path), Path: nm, In: w.Path,
				Does: "deletes it: git ignores it, and an install puts it back", Clean: removed(nm)})
		}
		return out
	},
}

func bunCache() string {
	if d := os.Getenv("BUN_INSTALL_CACHE_DIR"); d != "" {
		return d
	}
	home, _ := os.UserHomeDir()
	return filepath.Join(home, ".bun", "install", "cache")
}

var gitKit = &kit{
	name: "git-clean",
	about: "In the repos agents work in: worktrees whose folders are gone, local branches already merged, and what git gc would pack. " +
		"Agents' worktrees themselves are in Projects. Off until you turn it on; needs git.",
	needs: []string{"git"},
	things: func(ctx context.Context, seen []session) []thing {
		var out []thing
		for _, repo := range repos(ctx, seen) {
			name := filepath.Base(repo)
			if n := staleWorktrees(ctx, repo); n > 0 {
				out = append(out, thing{ID: "prune:" + repo, Title: name + ": worktrees whose folders are gone", Meta: plural(n, "entry", "entries"),
					Does: "runs git worktree prune", Clean: ran(repo, "git", "worktree", "prune")})
			}
			if base, merged := mergedBranches(ctx, repo); len(merged) > 0 {
				out = append(out, thing{ID: "merged:" + repo, Title: name + ": branches merged into " + base, Meta: plural(len(merged), "branch", "branches"),
					Does: "deletes them locally: " + clip(strings.Join(merged, ", "), 300), Clean: deleteBranches(repo, merged)})
			}
			out = append(out, thing{ID: "gc:" + repo, Title: name + ": git's objects", Path: filepath.Join(repo, ".git"),
				Does: "runs git gc: packs objects and drops unreachable ones older than two weeks", Clean: ran(repo, "git", "gc", "--quiet")})
		}
		return out
	},
}

// staleWorktrees counts a repo's worktrees whose folders are gone.
func staleWorktrees(ctx context.Context, repo string) int {
	out, err := command(ctx, repo, "git", "worktree", "list", "--porcelain")
	if err != nil {
		return 0
	}
	return countPrunable(out)
}

func countPrunable(porcelain string) int {
	n := 0
	for l := range strings.SplitSeq(porcelain, "\n") {
		if l == "prunable" || strings.HasPrefix(l, "prunable ") {
			n++
		}
	}
	return n
}

// mergedBranches are a repo's local branches whose work is on its
// remote's default branch, checked out nowhere: deleting them loses
// nothing. The rule is fleet's, every clean-up's: merged into it here or on
// origin, or their pull request merged, squashed too.
func mergedBranches(ctx context.Context, repo string) (base string, merged []string) {
	base, err := command(ctx, repo, "git", "symbolic-ref", "--short", "refs/remotes/origin/HEAD")
	if err != nil {
		return "", nil
	}
	prs, _ := ghpr.LookBranches(ctx, repo) // without gh, squash merges aren't known: the rest still are
	return base, fleet.LandedBranches(repo, strings.TrimPrefix(base, "origin/"), prs)
}

// deleteBranches deletes branches whose work is on the remote's default branch:
// those listed that still are, since one may have moved since. git branch
// -d would refuse any not merged into HEAD, which in a repo checked out
// elsewhere means nothing, so it's -D.
func deleteBranches(repo string, listed []string) func(context.Context) (string, error) {
	return func(ctx context.Context) (string, error) {
		_, now := mergedBranches(ctx, repo)
		names := slices.DeleteFunc(slices.Clone(listed), func(n string) bool { return !slices.Contains(now, n) })
		if len(names) == 0 {
			return "None is still merged.", nil
		}
		_, err := command(ctx, repo, "git", append([]string{"branch", "-D"}, names...)...)
		if err != nil {
			return "", err
		}
		return "Deleted " + plural(len(names), "branch", "branches") + ".", nil
	}
}

func plural(n int, one, many string) string {
	if n == 1 {
		return "1 " + one
	}
	return strconv.Itoa(n) + " " + many
}

func clip(s string, n int) string {
	if r := []rune(s); len(r) > n {
		return string(r[:n-1]) + "…"
	}
	return s
}
