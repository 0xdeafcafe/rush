package ollama

import (
	"context"
	"fmt"
	"net/http"
	"net/http/httptest"
	"slices"
	"strings"
	"testing"
	"unicode/utf8"

	"github.com/0xdeafcafe/photon/jsonx"
)

func TestSummaryModelsRejectClassifierEvenWithCompletion(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	calls := 0
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/api/version":
			fmt.Fprint(w, `{}`)
		case "/api/tags":
			fmt.Fprint(w, `{"models":[{"name":"tev1:4b"},{"name":"writer"},{"name":"embed"}]}`)
		case "/api/ps":
			fmt.Fprint(w, `{"models":[]}`)
		case "/api/show":
			var in struct{ Model string }
			jsonx.Decode(r.Body, &in)
			switch in.Model {
			case "tev1:4b":
				fmt.Fprint(w, `{"capabilities":["decision","tools","completion"]}`)
			case "writer":
				fmt.Fprint(w, `{"capabilities":["completion"]}`)
			case "embed":
				fmt.Fprint(w, `{"capabilities":["embedding"]}`)
			}
		case "/api/chat":
			calls++
			fmt.Fprint(w, `{"message":{"content":"A"}}`)
		}
	}))
	defer srv.Close()
	t.Setenv("OLLAMA_HOST", srv.URL)
	got, err := (Adapter{}).SummaryModels(context.Background())
	if err != nil || !slices.Equal(got, []string{"writer"}) {
		t.Fatalf("models=%v err=%v", got, err)
	}
	if why := strings.Join((Adapter{}).SummaryExclusions(), " "); !strings.Contains(why, "tev1:4b") || !strings.Contains(why, "decision classifier") {
		t.Fatal(why)
	}
	if _, err := (Adapter{}).Summarize(context.Background(), "tev1:4b", "summarize", "work"); err == nil || !strings.Contains(err.Error(), "decision classifier") {
		t.Fatalf("err=%v", err)
	}
	if calls != 0 {
		t.Fatal("classifier invoked as summarizer")
	}
}

func TestSummaryChunksPreserveEveryByteAndUTF8(t *testing.T) {
	input := strings.Repeat("重要な修正\n", 120) + "last task"
	chunks := summaryChunks(input, 43)
	if strings.Join(chunks, "") != input {
		t.Fatal("input was lost")
	}
	for _, chunk := range chunks {
		if len(chunk) > 43 || !utf8.ValidString(chunk) {
			t.Fatalf("bad chunk %q", chunk)
		}
	}
}

func TestSummarizeUsesBoundedContextAndIncludesTheMiddle(t *testing.T) {
	t.Setenv("RUSH_HOME", t.TempDir())
	original := strings.Repeat("first work. ", 2500) + "IMPORTANT MIDDLE TASK" + strings.Repeat("last work. ", 2500)
	var sent []string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/api/version":
			fmt.Fprint(w, `{}`)
		case "/api/show":
			fmt.Fprint(w, `{"capabilities":["completion"],"model_info":{"test.context_length":262144}}`)
		case "/api/chat":
			var in struct {
				Model    string
				Think    bool
				Options  map[string]int
				Messages []struct{ Role, Content string }
			}
			jsonx.Decode(r.Body, &in)
			if in.Options["num_ctx"] != 32768 || in.Options["num_predict"] != 4096 || in.Think {
				t.Errorf("options=%v think=%v", in.Options, in.Think)
			}
			sent = append(sent, in.Messages[1].Content)
			fmt.Fprint(w, `{"message":{"content":"Preserved the important pending work, all decisions and constraints."},"done_reason":"stop"}`)
		}
	}))
	defer srv.Close()
	t.Setenv("OLLAMA_HOST", srv.URL)
	out, err := (Adapter{}).Summarize(context.Background(), "writer", "summarize", original)
	if err != nil || out == "" {
		t.Fatalf("out=%q err=%v", out, err)
	}
	if len(sent) < 3 {
		t.Fatalf("expected chunks and combined summary, got %d calls", len(sent))
	}
	if strings.Join(sent[:len(sent)-1], "") != original {
		t.Fatal("not all original input reached the summarizer")
	}
}

func TestSummaryOutputLimitLeavesConversationUnchanged(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		fmt.Fprint(w, `{"message":{"content":"partial summary"},"done_reason":"length"}`)
	}))
	defer srv.Close()
	t.Setenv("OLLAMA_HOST", srv.URL)
	if out, err := summarizePart(context.Background(), "writer", "summary", "work", 8192, 2048); err == nil || out != "" {
		t.Fatalf("out=%q err=%v", out, err)
	}
}

func TestDecisionModelCannotRunCodingHarnessEvenWithTools(t *testing.T) {
	classifier := Model{Name: "tev1:4b", Capabilities: []string{"decision", "tools", "completion"}}
	if classifier.CanCode() {
		t.Fatal("decision classifier falsely offered for coding")
	}
	if !(Model{Capabilities: []string{"tools", "completion"}}).CanCode() {
		t.Fatal("ordinary coding model excluded")
	}
	t.Setenv("RUSH_HOME", t.TempDir())
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/api/version":
			fmt.Fprint(w, `{}`)
		case "/api/show":
			fmt.Fprint(w, `{"capabilities":["decision","tools","completion"]}`)
		default:
			t.Errorf("must refuse before loading: %s", r.URL.Path)
		}
	}))
	defer srv.Close()
	t.Setenv("OLLAMA_HOST", srv.URL)
	if _, err := pick(context.Background(), "tev1:4b"); err == nil || !strings.Contains(err.Error(), "decision classifier") {
		t.Fatalf("err=%v", err)
	}
}
