// Command probe is a plugin that tries to get out of its sandbox, for the
// sandbox tests. Asked to "probe", it attempts each thing it should not be
// able to do and says which it managed.
package main

import (
	"context"
	"encoding/json/jsontext"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"time"

	"github.com/0xdeafcafe/photon/jsonx"
	"github.com/0xdeafcafe/rush/internal/plugin"
)

func main() {
	c := plugin.NewConn(os.NewFile(3, "rush"), func(_ context.Context, method string, params jsontext.Value) (any, error) {
		var in struct {
			Secret, Outside, Sock, Data string
			Proxy                       int
		}
		_ = jsonx.Unmarshal(params, &in)
		ok := func(err error) string {
			if err != nil {
				return "denied: " + err.Error()
			}
			return "allowed"
		}
		dial := func(network, addr string) error {
			c, err := net.DialTimeout(network, addr, 2*time.Second)
			if err == nil {
				c.Close()
			}
			return err
		}
		out := map[string]string{}
		_, err := os.ReadFile(in.Secret)
		out["read secret"] = ok(err)
		out["write outside"] = ok(os.WriteFile(in.Outside, []byte("x"), 0o600))
		out["write data"] = ok(os.WriteFile(filepath.Join(in.Data, "x"), []byte("x"), 0o600))
		_, err = os.ReadDir(filepath.Dir(in.Secret))
		out["list secret dir"] = ok(err)
		out["exec"] = ok(exec.Command("/bin/echo", "hi").Run())
		out["internet"] = ok(dial("tcp", "1.1.1.1:443"))
		out["localhost"] = ok(dial("tcp", "127.0.0.1:22"))
		out["rush socket"] = ok(dial("unix", in.Sock))
		if in.Proxy > 0 {
			out["proxy"] = ok(dial("tcp", net.JoinHostPort("127.0.0.1", itoa(in.Proxy))))
		}
		out["home"] = os.Getenv("HOME")
		out["token"] = os.Getenv("ANTHROPIC_API_KEY")
		return out, nil
	})
	<-c.Done()
}

func itoa(n int) string {
	b, _ := jsonx.Marshal(n)
	return string(b)
}
