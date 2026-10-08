package event

import (
	"encoding/json/jsontext"
	"fmt"
	"reflect"

	"github.com/0xdeafcafe/photon/jsonx"
)

// names are each event's name on the wire.
var names = map[reflect.Type]string{}

// types are the events by their name on the wire.
var types = map[string]reflect.Type{}

func init() {
	for name, ev := range map[string]Event{
		"exchange": Exchange{}, "init": Init{}, "message_start": MessageStart{}, "part_start": PartStart{}, "delta": Delta{},
		"message": Message{}, "call_updated": CallUpdated{}, "approval": Approval{},
		"approval_cancelled": ApprovalCancelled{}, "denied": Denied{}, "question": Question{},
		"status": Status{}, "turn_end": TurnEnd{}, "compacted": Compacted{}, "quota": Quota{},
		"limited": Limited{}, "billing": Billing{}, "context": Context{}, "start_notice": StartNotice{}, "task_started": TaskStarted{},
		"task_updated": TaskUpdated{}, "task_progress": TaskProgress{}, "task_done": TaskDone{},
		"plan": Plan{}, "background": Background{}, "commands": Commands{}, "other": Other{},
	} {
		t := reflect.TypeOf(ev)
		names[t], types[name] = name, t
	}
}

// wire is an event as the host sends it: its name, and its fields.
type wire struct {
	T string         `json:"t"`
	E jsontext.Value `json:"e"`
}

// Marshal is ev as one line of JSON, which Unmarshal reads back.
func Marshal(ev Event) ([]byte, error) {
	name, ok := names[reflect.TypeOf(ev)]
	if !ok {
		return nil, fmt.Errorf("event: %T has no name", ev)
	}
	body, err := jsonx.Marshal(ev)
	if err != nil {
		return nil, err
	}
	return jsonx.Marshal(wire{T: name, E: body})
}

// Unmarshal reads an event Marshal wrote.
func Unmarshal(b []byte) (Event, error) {
	var w wire
	if err := jsonx.Unmarshal(b, &w); err != nil {
		return nil, err
	}
	t, ok := types[w.T]
	if !ok {
		return nil, fmt.Errorf("event: unknown %q", w.T)
	}
	v := reflect.New(t)
	if err := jsonx.Unmarshal(w.E, v.Interface()); err != nil {
		return nil, err
	}
	return v.Elem().Interface().(Event), nil
}
