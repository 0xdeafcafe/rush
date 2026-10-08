package jsonx

import json "encoding/json/v2"

// jsonx itself calls v2: that's the point of it.
func Marshal(v any) ([]byte, error) { return json.Marshal(v) }
