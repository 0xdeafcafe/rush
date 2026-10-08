package headless

import (
	"encoding/json/jsontext"

	"github.com/0xdeafcafe/photon/jsonx"
)

// Question is one of the questions Claude asks with the AskUserQuestion
// tool.
type Question struct {
	Question    string `json:"question"`
	Header      string `json:"header"`
	MultiSelect bool   `json:"multiSelect"`
	Options     []struct {
		Label       string `json:"label"`
		Description string `json:"description"`
		Preview     string `json:"preview"`
	} `json:"options"`
}

// Questions is what an AskUserQuestion request asks.
func (r *PermissionRequest) Questions() (title string, qs []Question) {
	var in struct {
		Title     string     `json:"title"`
		Questions []Question `json:"questions"`
	}
	_ = jsonx.Unmarshal(r.Input, &in)
	return in.Title, in.Questions
}

// AnswerInput is the tool input that answers r: Claude Code takes the
// answers keyed by question text, with the preview of each chosen option
// beside it. Questions left unanswered go without one.
func (r *PermissionRequest) AnswerInput(qs []Question, answers map[string]string) jsontext.Value {
	in := map[string]any{}
	_ = jsonx.Unmarshal(r.Input, &in)
	in["answers"] = answers
	notes := map[string]any{}
	for _, q := range qs {
		for _, o := range q.Options {
			if o.Preview != "" && answers[q.Question] == o.Label {
				notes[q.Question] = map[string]string{"preview": o.Preview}
			}
		}
	}
	if len(notes) > 0 {
		in["annotations"] = notes
	}
	b, _ := jsonx.Marshal(in)
	return b
}
