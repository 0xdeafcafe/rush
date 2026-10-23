package ts

import (
	"errors"
	"os"
	"path/filepath"
	"runtime"
)

// An Engine is a TypeScript language server: which one, and its binary.
type Engine struct {
	Name string // "tsrs" or "tsgo"
	Bin  string
}

// platform is the suffix npm's platform packages use: darwin-arm64,
// linux-x64…
func platform() string {
	arch := runtime.GOARCH
	if arch == "amd64" {
		arch = "x64"
	}
	return runtime.GOOS + "-" + arch
}

// globalModules are where npm and pnpm install packages for everyone,
// since the plugin's PATH holds only the system's folders.
func globalModules() []string {
	home, _ := os.UserHomeDir()
	return []string{
		"/opt/homebrew/lib/node_modules",
		"/usr/local/lib/node_modules",
		filepath.Join(home, ".npm-global", "lib", "node_modules"),
		filepath.Join(home, "Library", "pnpm", "global", "5", "node_modules"),
		filepath.Join(home, ".local", "share", "pnpm", "global", "5", "node_modules"),
	}
}

// Resolve is the engine for the project at root, as want ("auto", "tsrs"
// or "tsgo") asks: tsrs first for auto, the project's own install before a
// global one. The platform binary is run, never a package's Node shim, so
// the process rush watches is the server itself.
func Resolve(root, want string) (Engine, error) {
	if want != "tsgo" {
		if bin := find(root, tsrsAt); bin != "" {
			return Engine{Name: "tsrs", Bin: bin}, nil
		}
		if want == "tsrs" {
			return Engine{}, errors.New("no tsrs for " + platform() + ": npm i -g @maschwenk/tsrs, or set the engine to auto")
		}
	}
	if bin := find(root, tsgoAt); bin != "" {
		return Engine{Name: "tsgo", Bin: bin}, nil
	}
	return Engine{}, errors.New("no TypeScript language server: install @maschwenk/tsrs (npm i -g), " +
		"or @typescript/native-preview or typescript 7 in the project")
}

// find is the first binary at, given a node_modules folder, finds: in
// root's and each folder above it, then the global ones.
func find(root string, at func(modules string) string) string {
	for dir := root; ; {
		if bin := at(filepath.Join(dir, "node_modules")); bin != "" {
			return bin
		}
		parent := filepath.Dir(dir)
		if parent == dir {
			break
		}
		dir = parent
	}
	for _, m := range globalModules() {
		if bin := at(m); bin != "" {
			return bin
		}
	}
	return ""
}

// tsrsAt is tsrs's platform binary for modules/@maschwenk/tsrs: beside
// the package where pnpm keeps it, under it where npm nests it, or at the
// top of modules where npm hoists it.
func tsrsAt(modules string) string {
	pkg := "tsrs-" + platform()
	return firstFile(
		siblings(filepath.Join(modules, "@maschwenk", "tsrs"), filepath.Join("..", pkg, "tsrs"),
			filepath.Join("node_modules", "@maschwenk", pkg, "tsrs")),
		filepath.Join(modules, "@maschwenk", pkg, "tsrs"),
	)
}

// tsgoAt is tsgo's platform binary: the native preview's, or TypeScript
// 7's own (the same engine, released as tsc).
func tsgoAt(modules string) string {
	preview := "native-preview-" + platform()
	ts7 := "typescript-" + platform()
	return firstFile(
		siblings(filepath.Join(modules, "@typescript", "native-preview"), filepath.Join("..", preview, "lib", "tsgo"),
			filepath.Join("node_modules", "@typescript", preview, "lib", "tsgo")),
		filepath.Join(modules, "@typescript", preview, "lib", "tsgo"),
		siblings(filepath.Join(modules, "typescript"), filepath.Join("..", "@typescript", ts7, "lib", "tsc"),
			filepath.Join("node_modules", "@typescript", ts7, "lib", "tsc")),
		filepath.Join(modules, "@typescript", ts7, "lib", "tsc"),
	)
}

// siblings is rel, joined to the package at pkg with its links resolved,
// for each rel; "" when there's no package.
func siblings(pkg string, rels ...string) string {
	real, err := filepath.EvalSymlinks(pkg)
	if err != nil {
		return ""
	}
	for _, rel := range rels {
		if p := filepath.Join(real, rel); isFile(p) {
			return p
		}
	}
	return ""
}

func firstFile(paths ...string) string {
	for _, p := range paths {
		if p != "" && isFile(p) {
			return p
		}
	}
	return ""
}

func isFile(p string) bool {
	st, err := os.Stat(p)
	return err == nil && st.Mode().IsRegular() && st.Mode()&0o111 != 0
}

// WorkspaceRoot is the root of the checkout file is in: the nearest
// folder above it with a .git (a worktree's is a file), else its nearest
// tsconfig.json's; "" for neither. One server answers for the whole
// checkout: opened files find their own projects inside it, and share
// what they load, where a server per tsconfig would load the shared
// packages once each.
func WorkspaceRoot(file string) string {
	dir := file
	if st, err := os.Stat(file); err != nil || !st.IsDir() {
		dir = filepath.Dir(file)
	}
	tsconfig := ""
	for {
		if _, err := os.Stat(filepath.Join(dir, ".git")); err == nil {
			return dir
		}
		if tsconfig == "" {
			if _, err := os.Stat(filepath.Join(dir, "tsconfig.json")); err == nil {
				tsconfig = dir
			}
		}
		parent := filepath.Dir(dir)
		if parent == dir {
			return tsconfig
		}
		dir = parent
	}
}
