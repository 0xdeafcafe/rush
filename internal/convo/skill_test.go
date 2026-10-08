package convo

import (
	"strings"
	"testing"

	"github.com/0xdeafcafe/photon/cellw"
	"github.com/0xdeafcafe/rush/internal/host"
)

func TestSkillReadPresentation(t *testing.T) {
	for _, via := range []string{"Read", "Bash", "Skill"} {
		t.Run(via, func(t *testing.T) {
			s := New()
			s.Apply(host.Sent{Text: "review the design"}, at(0))
			input := map[string]any{"file_path": "/skills/design-critique/SKILL.md"}
			if via == "Bash" {
				input = map[string]any{"command": "cat /skills/design-critique/SKILL.md"}
			}
			if via == "Skill" {
				input = map[string]any{"skill": "design-critique"}
			}
			s.Apply(toolUse("skill", via, input), at(1))
			o := Options{Width: 90, Now: at(2)}
			if out := plain(s.Render(o)); !strings.Contains(out, "Loading skill") || strings.Contains(out, "Loaded skill") {
				t.Fatalf("pending skill state: %s", out)
			}
			content := "---\nname: design-critique\ndescription: >-\n  Review usability and hierarchy.\n  Suggest specific improvements.\n---\nSECRET_BODY_SENTINEL\n"
			s.Apply(toolResult("skill", content, false, nil), at(3))
			o.Now = at(4)
			lines := s.Render(o)
			out := plain(lines)
			for _, want := range []string{"Loaded skill", "design-critique", "Review usability and hierarchy.", "Suggest specific improvements."} {
				if !strings.Contains(out, want) {
					t.Fatalf("missing %q: %s", want, out)
				}
			}
			if strings.Contains(out, "SECRET_BODY_SENTINEL") || strings.Contains(out, "SKILL.md") {
				t.Fatalf("skill output should be collapsed: %s", out)
			}
			ref := ""
			for _, l := range lines {
				if strings.HasSuffix(l.Ref, ":s:skill") {
					ref = l.Ref
				}
			}
			if ref == "" || s.StepOpen(ref, false) {
				t.Fatal("skill row must be expandable and initially closed")
			}
			o.Open = map[string]bool{ref: true}
			if out := plain(s.Render(o)); !strings.Contains(out, "SECRET_BODY_SENTINEL") {
				t.Fatalf("expanded skill lost original output: %s", out)
			}
			o.Open = nil
			o.Width = 36
			for _, line := range s.Render(o) {
				if cellw.String(line.Text) > o.Width {
					t.Fatalf("skill overflows narrow layout: %q", line.Text)
				}
			}
		})
	}
}

func TestSkillFailureAndOrdinaryRead(t *testing.T) {
	for _, path := range []string{"/skills/review/SKILL.md", "/docs/README.md"} {
		s := New()
		s.Apply(host.Sent{Text: "read instructions"}, at(0))
		s.Apply(toolUse("r", "Read", map[string]any{"file_path": path}), at(1))
		s.Apply(toolResult("r", "permission denied", true, nil), at(2))
		out := plain(s.Render(Options{Width: 90, Now: at(3)}))
		if !strings.Contains(out, "permission denied") || strings.Contains(out, "Loaded skill") {
			t.Fatalf("lost failure: %s", out)
		}
		if strings.Contains(out, "Couldn't load skill") != strings.HasSuffix(path, "SKILL.md") {
			t.Fatalf("incorrect classification: %s", out)
		}
	}
}

func TestMixedSkillCommandKeepsOutput(t *testing.T) {
	s := New()
	s.Apply(host.Sent{Text: "read files"}, at(0))
	s.Apply(toolUse("r", "Bash", map[string]any{"command": "cat /skills/review/SKILL.md /docs/README.md"}), at(1))
	s.Apply(toolResult("r", "ordinary output", false, nil), at(2))
	out := plain(s.Render(Options{Width: 90, Now: at(3)}))
	if strings.Contains(out, "Loaded skill") || !strings.Contains(out, "ordinary output") {
		t.Fatalf("mixed command output hidden: %s", out)
	}
}
