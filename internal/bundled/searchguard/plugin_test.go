package searchguard

import (
	"strconv"
	"strings"
	"testing"
)

func TestRefusal(t *testing.T) {
	alts := func(sep string, n int) string {
		w := make([]string, n)
		for i := range w {
			w[i] = "name" + strconv.Itoa(i)
		}
		return strings.Join(w, sep)
	}
	for _, c := range []struct {
		command, want string
	}{
		// Recursive grep.
		{"grep -rn foo .", recursiveGrep},
		{"grep -r foo src", recursiveGrep},
		{"grep -R foo", recursiveGrep},
		{"grep -nR foo .", recursiveGrep},
		{"grep -i --recursive foo .", recursiveGrep},
		{"grep -d recurse foo .", recursiveGrep},
		{"cd app && grep -rln 'x' . | head", recursiveGrep},
		{"LC_ALL=C grep -rn foo .", recursiveGrep},
		{"egrep -r 'a|b' .", recursiveGrep},
		// Giant alternations.
		{"git grep -n '" + alts(`\|`, 16) + "'", bigAlternation},
		{`rg -n "` + alts("|", 20) + `" src`, bigAlternation},
		{"git -C repo grep -nE '" + alts("|", 16) + "'", bigAlternation},
		// One whole-tree git grep per name.
		{"for n in a b c; do git grep -n \"$n\"; done", grepPerName},
		{"while read n; do echo $n; git grep -l \"$n\" | wc -l; done < names.txt", grepPerName},
		{"cat names | xargs -n1 git grep -n", grepPerName},
		{`node -e 'names.map(n => execFileSync("git", ["grep", "-n", n]))'`, grepPerName},
		{"node -e 'for (const n of names) execSync(`git grep -l ${n}`)'", grepPerName},
		{`python3 -c 'for n in names: subprocess.run(["git", "grep", n])'`, grepPerName},
		// Fine.
		{"rg -n foo", ""},
		{"git grep -n foo", ""},
		{"grep foo file.txt", ""},
		{"ps | grep x", ""},
		{"ps aux | grep -i node", ""},
		{"grep -e -r file.txt", ""},
		{"grep -A 3 -B2 foo file.txt", ""},
		{"grep --color=never foo file.txt", ""},
		{"rg -n -F -w -f words.txt", ""},
		{"git grep -nE 'a|b|c|d'", ""},
		{"grep -E '" + alts("|", 30) + "' file.txt", ""},
		{"for f in a b; do git grep -n foo -- $f; done", ""},
		{"for f in *.go; do echo $f; done; git grep -n foo", ""},
		{"for f in *.go; do rg -n foo $f; done", ""},
		{`node -e 'execFileSync("git", ["grep", "-n", "foo"])'`, ""},
		{"echo 'grep -r is slow'", ""},
		{"git log --grep foo", ""},
	} {
		if got := Refusal(c.command); got != c.want {
			t.Errorf("Refusal(%q) = %q, want %q", c.command, got, c.want)
		}
	}
}
