package plugin

type Conn struct{}

func (c *Conn) Call(method string) error   { return nil }
func (c *Conn) Notify(method string) error { return nil }

func DialBroker() (*Conn, error) { return &Conn{}, nil }

func CleanNotice(s string) string { return s }
