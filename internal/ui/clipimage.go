package ui

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"
)

// setClipboard puts text on the clipboard through the terminal (OSC 52),
// and on a Mac rush runs on, through pbcopy as well: Terminal.app ignores
// OSC 52, so a copy there would otherwise go nowhere.
func setClipboard(text string) tea.Cmd {
	osc := tea.SetClipboard(text)
	if runtime.GOOS != "darwin" || os.Getenv("SSH_CONNECTION") != "" || os.Getenv("SSH_TTY") != "" {
		return osc
	}
	return tea.Batch(osc, func() tea.Msg {
		c := exec.Command("pbcopy")
		c.Stdin = strings.NewReader(text)
		_ = c.Run()
		return nil
	})
}

// clipImageMsg is an image read off the clipboard and saved to a file;
// path is empty when the clipboard held none.
type clipImageMsg struct {
	path string
	err  error
}

// pasteClipImage reads an image off the clipboard, as ctrl+v does in Claude
// Code: a terminal pastes only text, so a screenshot copied to the clipboard
// never reaches rush as a paste. Reading it can take a moment, so the
// status line spins until it's back.
func (m *Model) pasteClipImage() tea.Cmd {
	read := func() tea.Msg {
		path, err := clipImage()
		return clipImageMsg{path: path, err: err}
	}
	if m.pastingImg {
		return read // a spin is already going
	}
	m.pastingImg = true
	return tea.Batch(read, pasteSpin())
}

// pasteSpinMsg turns the clipboard spinner a frame, quicker than the
// app's own tick.
type pasteSpinMsg struct{}

func pasteSpin() tea.Cmd {
	return tea.Tick(100*time.Millisecond, func(time.Time) tea.Msg { return pasteSpinMsg{} })
}

// clipImage is the image on the clipboard as a file: an image file copied in
// the Finder as itself, image data saved as a PNG under the temp dir. It is
// "" with no error when the clipboard holds no image.
func clipImage() (string, error) {
	dir := filepath.Join(os.TempDir(), "rush-images")
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return "", err
	}
	out := filepath.Join(dir, fmt.Sprintf("clipboard-%s.png", time.Now().Format("20060102-150405.000")))
	switch runtime.GOOS {
	case "darwin":
		// A copied file: attach the file itself when it's an image.
		if b, err := exec.Command("osascript", "-e", "POSIX path of (the clipboard as «class furl»)").Output(); err == nil {
			if p := filePath(strings.TrimSpace(string(b))); isImageFile(p) {
				if st, err := os.Stat(p); err == nil && !st.IsDir() {
					return p, nil
				}
			}
		}
		script := []string{
			"-e", "set png to (the clipboard as «class PNGf»)",
			"-e", fmt.Sprintf("set f to open for access POSIX file %q with write permission", out),
			"-e", "write png to f",
			"-e", "close access f",
		}
		if exec.Command("osascript", script...).Run() != nil {
			os.Remove(out)
			return "", nil // no image data on the clipboard
		}
	default:
		var data []byte
		var err error
		if os.Getenv("WAYLAND_DISPLAY") != "" {
			data, err = exec.Command("wl-paste", "--type", "image/png").Output()
		} else {
			data, err = exec.Command("xclip", "-selection", "clipboard", "-t", "image/png", "-o").Output()
		}
		if err != nil || len(data) == 0 {
			return "", nil
		}
		if err := os.WriteFile(out, data, 0o600); err != nil {
			return "", err
		}
	}
	if st, err := os.Stat(out); err != nil || st.Size() == 0 {
		os.Remove(out)
		return "", nil
	}
	return out, nil
}

// attachImages adds image files to whichever box has focus, each as
// [Image #N] at the cursor.
func (m *Model) attachImages(imgs []string) {
	if c := m.host; c != nil && m.paneFocus {
		c.undo.save(c.input, c.back, false)
		c.input = withMarks(c.input, max(0, len(c.input)-c.back), &c.imgs, imgs)
		return
	}
	m.undo.save(m.input, m.back, false)
	m.input = withMarks(m.input, m.cursorPos(), &m.imgs, imgs)
}

// withMarks is buf with a marker for each image put in at pos, spaced off
// the words either side.
func withMarks(buf []rune, pos int, r *imageRefs, imgs []string) []rune {
	marks := make([]string, 0, len(imgs))
	for _, p := range imgs {
		marks = append(marks, r.add(p))
	}
	ins := strings.Join(marks, " ")
	if pos > 0 && buf[pos-1] != ' ' && buf[pos-1] != '\n' {
		ins = " " + ins
	}
	if pos < len(buf) && buf[pos] != ' ' {
		ins += " "
	}
	return insert(buf, pos, []rune(ins))
}
