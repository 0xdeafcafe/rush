package lint

import (
	"go/ast"
	"strconv"
	"strings"

	"golang.org/x/tools/go/analysis"
)

// JSON keeps rush on json v2, through photon/jsonx: v1 is banned, and
// v2's Marshal and Unmarshal are only called by jsonx, so every call gets
// jsonx.Opts (what keeps other programs' JSON readable).
var JSON = &analysis.Analyzer{
	Name: "rushjson",
	Doc:  "bans encoding/json (v1), and calling json v2 other than through photon/jsonx",
	Run:  runJSON,
}

var jsonCalls = map[string]map[string]bool{
	"encoding/json/v2":       {"Marshal": true, "MarshalWrite": true, "MarshalEncode": true, "Unmarshal": true, "UnmarshalRead": true, "UnmarshalDecode": true},
	"encoding/json/jsontext": {"NewDecoder": true, "NewEncoder": true},
}

func runJSON(pass *analysis.Pass) (any, error) {
	inJSONX := strings.HasSuffix(pass.Pkg.Path(), "/photon/jsonx")
	for _, f := range pass.Files {
		for _, im := range f.Imports {
			if p, _ := strconv.Unquote(im.Path.Value); p == "encoding/json" {
				pass.Reportf(im.Pos(), "encoding/json is json v1: use photon/jsonx (json v2)")
			}
		}
		if inJSONX {
			continue
		}
		ast.Inspect(f, func(n ast.Node) bool {
			call, ok := n.(*ast.CallExpr)
			if !ok {
				return true
			}
			if pkg, name, _ := callee(pass.TypesInfo, call); jsonCalls[pkg][name] {
				pass.Reportf(call.Pos(), "call jsonx rather than %s.%s, so the call gets jsonx.Opts", pkg[strings.LastIndex(pkg, "/")+1:], name)
			}
			return true
		})
	}
	return nil, nil
}
