// Command shots takes the README's screenshots of rush on a made-up
// world: Sam's repositories and the agents that worked in them, which
// belong to nobody. It runs the installed rush in a tmux server of its
// own, with a home of the world's (never the real one), presses the keys
// that reach each screen, and draws what tmux shows as a picture in a
// macOS window.
//
//	go -C tools/shots run . [-only name] [../../docs/screenshots]
//
// It needs tmux and Claude Code installed (rush lists Claude Code's
// sessions only when it is; it's never run), and writes WebP with cwebp
// when that's installed, else PNG.
package main

import (
	"flag"
	"fmt"
	"image/png"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"

	"github.com/charmbracelet/x/ansi"
)

func main() {
	only := flag.String("only", "", "take only the shots whose names contain this")
	rush := flag.String("rush", "", "the rush to run (default: the one on PATH)")
	root := flag.String("root", filepath.Join(os.TempDir(), "rush-shots"), "where the world is built, afresh for each shot")
	scale := flag.Float64("scale", 2, "pixels per point")
	keepPNG := flag.Bool("png", false, "write PNG even when cwebp is installed")
	keep := flag.Bool("keep", false, "leave rush running in tmux after the last shot, to look around: tmux -L rush-shots attach")
	ansiFile := flag.String("ansi", "", "draw this file of ANSI text as a picture beside it, and nothing else")
	flag.Parse()
	if err := run(*only, *rush, *root, *scale, *keepPNG, *keep, *ansiFile); err != nil {
		fmt.Fprintln(os.Stderr, "shots:", err)
		os.Exit(1)
	}
}

func run(only, rush, root string, scale float64, keepPNG, keep bool, ansiFile string) error {
	fs, err := loadFonts(13 * scale)
	if err != nil {
		return err
	}
	if ansiFile != "" {
		b, err := os.ReadFile(ansiFile)
		if err != nil {
			return err
		}
		text := strings.TrimRight(string(b), "\n")
		w := 0
		for _, l := range strings.Split(text, "\n") {
			w = max(w, ansi.StringWidth(l))
		}
		return save(fs, scale, text, w, strings.Count(text, "\n")+1, strings.TrimSuffix(ansiFile, filepath.Ext(ansiFile)), true)
	}
	out := "../../docs/screenshots"
	if flag.NArg() > 0 {
		out = flag.Arg(0)
	}
	if out, err = filepath.Abs(out); err != nil {
		return err
	}
	if rush == "" {
		if rush, err = exec.LookPath("rush"); err != nil {
			return fmt.Errorf("no rush on PATH; say which with -rush: %w", err)
		}
	}
	webp := false
	if _, err := exec.LookPath("cwebp"); err == nil && !keepPNG {
		webp = true
	}
	for _, s := range shots {
		if only != "" && !strings.Contains(s.name, only) {
			continue
		}
		start := time.Now()
		frame, err := take(s, rush, root, keep)
		if err != nil {
			return fmt.Errorf("%s: %w", s.name, err)
		}
		_ = os.WriteFile(filepath.Join(root, s.name+".ans"), []byte(frame), 0o644)
		if err := save(fs, scale, frame, s.w, s.h, filepath.Join(out, s.name), !webp); err != nil {
			return fmt.Errorf("%s: %w", s.name, err)
		}
		fmt.Printf("%-20s %s\n", s.name, time.Since(start).Round(100*time.Millisecond))
	}
	return nil
}

// take builds the world, runs rush in it, goes through the shot's steps
// and says what the terminal shows at the end.
func take(s shot, rush, root string, keep bool) (string, error) {
	(&term{sock: "rush-shots"}).stop() // one left running with -keep
	w, err := build(root)
	if err != nil {
		return "", err
	}
	program, err := w.script(rush)
	if err != nil {
		return "", err
	}
	t, err := start("rush-shots", program, s.w, s.h)
	if err != nil {
		return "", err
	}
	if !keep {
		defer t.stop()
	}
	if err := t.until("AGENTS", 15*time.Second); err != nil {
		return "", err
	}
	time.Sleep(1500 * time.Millisecond) // the list's first reading
	for _, step := range s.steps {
		if err := t.do(step); err != nil {
			return "", err
		}
	}
	return t.screen(true), nil
}

// save draws the frame in a window and writes it as path.png, or as
// path.webp through cwebp.
func save(fs *fonts, scale float64, frame string, w, h int, path string, asPNG bool) error {
	img := window(fs.paint(screen(strings.TrimRight(frame, "\n"), w, h), w, h), "rush", fs, scale)
	pngPath := path + ".png"
	if !asPNG {
		pngPath = filepath.Join(os.TempDir(), filepath.Base(path)+".png")
	}
	f, err := os.Create(pngPath)
	if err != nil {
		return err
	}
	if err := png.Encode(f, img); err != nil {
		f.Close()
		return err
	}
	if err := f.Close(); err != nil || asPNG {
		return err
	}
	defer os.Remove(pngPath)
	if out, err := exec.Command("cwebp", "-quiet", "-q", "80", pngPath, "-o", path+".webp").CombinedOutput(); err != nil {
		return fmt.Errorf("cwebp: %v: %s", err, out)
	}
	return nil
}
