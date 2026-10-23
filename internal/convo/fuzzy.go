package convo

import (
	"strings"
	"unicode"
	"unicode/utf8"
)

// A word of a query matches text loosely: anywhere in it, as with find, or,
// failing that, as a word of the text a letter or two off (a typo). Each
// match scores how close it is, so the best of many come first.

// Scores of a word's match: a whole word beats the start of one, which
// beats the middle of one, which beats a typo.
const (
	scoreWhole  = 100
	scoreStart  = 80
	scoreInside = 60
	scoreTypo   = 40 // less 10 for each letter off past the first
	// scorePhrase is the words, as typed, side by side in the text.
	scorePhrase = 50
)

// typoScan is how much of a text is read for typos: past it a word is only
// found as it was typed.
const typoScan = 64 << 10

// typos is how many letters a word may be off by: none when short, so
// "fix" doesn't find "fox", two when long.
func typos(w string) int {
	switch n := utf8.RuneCountInString(w); {
	case n < 4:
		return 0
	case n < 8:
		return 1
	}
	return 2
}

// wordMatch is how well w matches text, and the byte span in text of the
// match; 0 and -1, -1 when it doesn't.
func wordMatch(text, w string) (score, start, end int) {
	if i, j := findFold(text, w); i >= 0 {
		score = scoreInside
		before, _ := utf8.DecodeLastRuneInString(text[:i])
		after, _ := utf8.DecodeRuneInString(text[j:])
		if i == 0 || !wordRune(before) {
			score = scoreStart
			if j == len(text) || !wordRune(after) {
				score = scoreWhole
			}
		}
		return score, i, j
	}
	k := typos(w)
	if k == 0 {
		return 0, -1, -1
	}
	hay := text[:min(len(text), typoScan)]
	lw := []rune(strings.ToLower(w))
	best, bi, bj := k+1, -1, -1
	for i := 0; i < len(hay); {
		r, n := utf8.DecodeRuneInString(hay[i:])
		if !wordRune(r) {
			i += n
			continue
		}
		j := i
		for j < len(hay) {
			r, n := utf8.DecodeRuneInString(hay[j:])
			if !wordRune(r) {
				break
			}
			j += n
		}
		if d := distance(lw, []rune(strings.ToLower(hay[i:j])), k); d < best {
			best, bi, bj = d, i, j
			if d == 1 {
				break
			}
		}
		i = j
	}
	if bi < 0 {
		return 0, -1, -1
	}
	return scoreTypo - 10*(best-1), bi, bj
}

func wordRune(r rune) bool { return unicode.IsLetter(r) || unicode.IsDigit(r) || r == '_' }

// distance is how many letters a and b are apart (added, dropped, changed
// or two swapped), or more than k when that's all that's known.
func distance[T rune | byte](a, b []T, k int) int {
	if d := len(a) - len(b); d > k || -d > k {
		return k + 1
	}
	// Three rows of the table, two back for a swap; on the stack when the
	// word is short, as nearly all are.
	var r0, r1, r2 [33]int
	prev2, prev, cur := r0[:], r1[:], r2[:]
	if len(b)+1 > len(r0) {
		prev2, prev, cur = make([]int, len(b)+1), make([]int, len(b)+1), make([]int, len(b)+1)
	}
	for j := 0; j <= len(b); j++ {
		prev[j] = j
	}
	for i := 1; i <= len(a); i++ {
		cur[0] = i
		low := cur[0]
		for j := 1; j <= len(b); j++ {
			cost := 1
			if a[i-1] == b[j-1] {
				cost = 0
			}
			cur[j] = min(prev[j]+1, cur[j-1]+1, prev[j-1]+cost)
			if i > 1 && j > 1 && a[i-1] == b[j-2] && a[i-2] == b[j-1] {
				cur[j] = min(cur[j], prev2[j-2]+1)
			}
			low = min(low, cur[j])
		}
		if low > k {
			return k + 1
		}
		prev2, prev, cur = prev, cur, prev2
	}
	return prev[len(b)]
}

// MatchSpan is where w matches s, as a search finds it (a typo too), as a
// byte span of s; -1, -1 when it doesn't.
func MatchSpan(s, w string) (start, end int) {
	if strings.TrimSpace(w) == "" {
		return -1, -1
	}
	_, i, j := wordMatch(s, w)
	return i, j
}
