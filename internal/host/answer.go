package host

import (
	"encoding/json/jsontext"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/agent/event"
)

// AnswerInput is the input that answers q through Client.Allow: the
// questions as Claude Code's AskUserQuestion takes them, with the answers
// keyed by question text and the preview of each chosen option beside it.
// A host from before rush's own events hands it to Claude Code as it is;
// today's takes the answers from it, whichever agent asked. Questions left
// unanswered go without one.
func AnswerInput(q *event.Question, answers map[string]string) jsontext.Value {
	type option struct {
		Label       string `json:"label"`
		Description string `json:"description"`
		Preview     string `json:"preview,omitempty"`
	}
	type ask struct {
		Question    string   `json:"question"`
		Header      string   `json:"header"`
		MultiSelect bool     `json:"multiSelect"`
		Options     []option `json:"options"`
	}
	in := struct {
		Title       string            `json:"title,omitempty"`
		Questions   []ask             `json:"questions"`
		Answers     map[string]string `json:"answers"`
		Annotations map[string]any    `json:"annotations,omitempty"`
	}{Title: q.Title, Questions: []ask{}, Answers: answers}
	notes := map[string]any{}
	for _, a := range q.Asks {
		x := ask{Question: a.Text, Header: a.Header, MultiSelect: a.Multi, Options: []option{}}
		for _, o := range a.Options {
			x.Options = append(x.Options, option(o))
			if o.Preview != "" && answers[a.Text] == o.Label {
				notes[a.Text] = map[string]string{"preview": o.Preview}
			}
		}
		in.Questions = append(in.Questions, x)
	}
	if len(notes) > 0 {
		in.Annotations = notes
	}
	b, _ := jsonx.Marshal(in)
	return b
}
