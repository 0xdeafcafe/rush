package host

import "github.com/0xdeafcafe/photon/jsonx"

// Op sends one op a remote client wrote as JSON, as the host's own client
// methods would, with images (files on this machine its server wrote) in
// place of any it named. It is checked only for being an op: which ops a
// remote client may send is its server's to say.
func (c *Client) Op(raw []byte, images ...string) error {
	var o op
	if err := jsonx.Unmarshal(raw, &o); err != nil {
		return err
	}
	o.Images = images
	return c.do(o)
}

// OpName is the op raw names, "" when it isn't one.
func OpName(raw []byte) string {
	var o struct {
		Op string `json:"op"`
	}
	_ = jsonx.Unmarshal(raw, &o)
	return o.Op
}
