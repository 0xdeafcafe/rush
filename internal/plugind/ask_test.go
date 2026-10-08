package plugind

import (
	"context"
	"encoding/json/jsontext"
	"testing"

	"github.com/0xdeafcafe/rush/internal/jsonx"
	"github.com/0xdeafcafe/rush/internal/plugin"
)

// A plugin may ask about a message: the chain stops at its question, with
// what the plugins before it changed, and the key chosen goes back to it.
// What it then says changes the message, and the plugins after it are
// asked as usual.
func TestUIInterceptAsk(t *testing.T) {
	b := testBroker(t)
	intercept := []string{plugin.UIInput, plugin.UIIntercept}
	var answered plugin.InterceptAnswer
	addPlugin(t, b, &plugin.Manifest{Name: "a", UI: intercept}, func(_ context.Context, method string, params jsontext.Value) (any, error) {
		var in plugin.Intercept
		_ = jsonx.Unmarshal(params, &in)
		return plugin.InterceptResult{Action: "rewrite", Replace: []plugin.Replacement{{Old: "hi", New: "hello"}}}, nil
	})
	addPlugin(t, b, &plugin.Manifest{Name: "b", UI: intercept}, func(_ context.Context, method string, params jsontext.Value) (any, error) {
		switch method {
		case "ui.intercept":
			return plugin.InterceptResult{Action: "ask", ID: "q1", Question: "Really\x1b?", Detail: "it says hello",
				Choices: []plugin.AskChoice{{Key: "y", Label: "yes", Enter: true}, {Key: "n", Label: "no", Esc: true}}}, nil
		case "ui.intercept.answer":
			_ = jsonx.Unmarshal(params, &answered)
			if answered.Key == "n" {
				return plugin.InterceptResult{Action: "block", Reason: "you said no"}, nil
			}
			return plugin.InterceptResult{Action: "rewrite", Append: "\n\nsigned"}, nil
		}
		return map[string]any{}, nil
	})
	addPlugin(t, b, &plugin.Manifest{Name: "c", UI: intercept}, func(_ context.Context, method string, params jsontext.Value) (any, error) {
		var in plugin.Intercept
		_ = jsonx.Unmarshal(params, &in)
		return plugin.InterceptResult{Action: "rewrite", Text: in.Text + "!"}, nil
	})
	u := attachUI(t, b, "main")

	var ask plugin.InterceptResult
	if err := u.call("ui.intercept", plugin.Intercept{Hook: "before-send", UI: "main", Box: "s1", Text: "hi there"}, &ask); err != nil {
		t.Fatal(err)
	}
	if ask.Action != "ask" || ask.Plugin != "b" || ask.ID != "q1" || ask.Question != "Really?" || ask.Text != "hello there" ||
		len(ask.Replace) != 1 || len(ask.Choices) != 2 {
		t.Fatalf("ask = %+v", ask)
	}
	var res plugin.InterceptResult
	if err := u.call("ui.intercept.answer", plugin.InterceptAnswer{Intercept: plugin.Intercept{Hook: "before-send", UI: "main", Box: "s1", Text: ask.Text},
		Plugin: "b", ID: ask.ID, Key: "y"}, &res); err != nil {
		t.Fatal(err)
	}
	if answered.ID != "q1" || answered.Key != "y" || answered.Text != "hello there" || answered.Box != "s1" {
		t.Fatalf("b was handed %+v", answered)
	}
	if res.Action != "rewrite" || res.Text != "hello there\n\nsigned!" || res.Plugin != "b, c" {
		t.Fatalf("after the answer = %+v", res)
	}
	if err := u.call("ui.intercept.answer", plugin.InterceptAnswer{Intercept: plugin.Intercept{Text: ask.Text}, Plugin: "b", ID: "q1", Key: "n"}, &res); err != nil {
		t.Fatal(err)
	}
	if res.Action != "block" || res.Plugin != "b" || res.Reason != "you said no" {
		t.Fatalf("a no = %+v", res)
	}
	// An answer for a plugin that isn't there holds the message back.
	if err := u.call("ui.intercept.answer", plugin.InterceptAnswer{Intercept: plugin.Intercept{Text: "x"}, Plugin: "gone", Key: "y"}, &res); err != nil {
		t.Fatal(err)
	}
	if res.Action != "block" {
		t.Fatalf("an answer nobody took = %+v", res)
	}
}

// An ask rush can't show is taken as allow.
func TestUIInterceptBadAsk(t *testing.T) {
	b := testBroker(t)
	addPlugin(t, b, &plugin.Manifest{Name: "a", UI: []string{plugin.UIInput, plugin.UIIntercept}}, func(context.Context, string, jsontext.Value) (any, error) {
		return plugin.InterceptResult{Action: "ask", Question: "?", Choices: []plugin.AskChoice{{Key: "Yes", Label: "y"}}}, nil
	})
	u := attachUI(t, b, "main")
	var res plugin.InterceptResult
	if err := u.call("ui.intercept", plugin.Intercept{Text: "hi"}, &res); err != nil || res.Action != "allow" {
		t.Fatalf("got %+v, %v", res, err)
	}
}

// An ask may be a line to type: it reaches the window with what the line
// starts as, and what was typed goes back to the plugin with enter, as
// does what the window can show.
func TestUIInterceptAskInput(t *testing.T) {
	b := testBroker(t)
	var asked plugin.Intercept
	var answered plugin.InterceptAnswer
	addPlugin(t, b, &plugin.Manifest{Name: "a", UI: []string{plugin.UIInput, plugin.UIIntercept}}, func(_ context.Context, method string, params jsontext.Value) (any, error) {
		switch method {
		case "ui.intercept":
			_ = jsonx.Unmarshal(params, &asked)
			return plugin.InterceptResult{Action: "ask", ID: "q1", Question: "Name it",
				Input:   &plugin.AskLine{Value: "MY\nKEY\x1b", Error: "taken", Enter: "save"},
				Choices: []plugin.AskChoice{{Key: "b", Label: "back", Esc: true}}}, nil
		case "ui.intercept.answer":
			_ = jsonx.Unmarshal(params, &answered)
			return plugin.InterceptResult{Action: "rewrite", Text: "sent as " + answered.Value}, nil
		}
		return map[string]any{}, nil
	})
	u := attachUI(t, b, "main")

	in := plugin.Intercept{Hook: "before-send", UI: "main", Box: "s1", Text: "hi", Asks: []string{plugin.AskInput}}
	var ask plugin.InterceptResult
	if err := u.call("ui.intercept", in, &ask); err != nil {
		t.Fatal(err)
	}
	if len(asked.Asks) != 1 || asked.Asks[0] != plugin.AskInput {
		t.Fatalf("the plugin is told what the window shows: %+v", asked)
	}
	if ask.Action != "ask" || ask.Input == nil || ask.Input.Value != "MY KEY" || ask.Input.Error != "taken" || ask.Input.Enter != "save" || len(ask.Choices) != 1 {
		t.Fatalf("ask = %+v, input %+v", ask, ask.Input)
	}
	var res plugin.InterceptResult
	if err := u.call("ui.intercept.answer", plugin.InterceptAnswer{Intercept: in, Plugin: "a", ID: ask.ID, Key: plugin.KeyEnter, Value: "OTHER\nNAME"}, &res); err != nil {
		t.Fatal(err)
	}
	if answered.Key != plugin.KeyEnter || answered.Value != "OTHER NAME" || len(answered.Asks) != 1 {
		t.Fatalf("the plugin was handed %+v", answered)
	}
	if res.Action != "rewrite" || res.Text != "sent as OTHER NAME" {
		t.Fatalf("after the answer = %+v", res)
	}
}

// A line to type has no keys of its own but esc: letters are typed.
func TestUIInterceptBadAskInput(t *testing.T) {
	for _, choices := range [][]plugin.AskChoice{
		{{Key: "y", Label: "yes"}},
		{{Key: "y", Label: "yes", Enter: true, Esc: true}},
		{{Key: "b", Label: "back", Esc: true}, {Key: "n", Label: "no"}},
	} {
		r := plugin.InterceptResult{Action: "ask", Question: "Name it", Input: &plugin.AskLine{}, Choices: choices}
		if plugin.CleanAsk(&r) == nil {
			t.Errorf("%+v was taken", choices)
		}
	}
	r := plugin.InterceptResult{Action: "ask", Question: "Name it", Input: &plugin.AskLine{}}
	if err := plugin.CleanAsk(&r); err != nil {
		t.Errorf("a line with no choice at all: %v", err)
	}
}
