package community

import "hash/fnv"

// An agent's feed name is a dog's name and then a dog's, cat's or bird's,
// picked from its stable session id, so it never changes: @biscuit-heron.
var (
	dogs = []string{
		"biscuit", "rex", "bean", "pickle", "waffle", "noodle", "rufus", "bramble",
		"pepper", "scout", "toffee", "barley", "ziggy", "mochi", "nugget", "pudding",
		"buster", "clover", "dumpling", "fudge", "gizmo", "juniper", "kipper", "marmite",
		"otis", "pippin", "rolo", "sprout", "turnip", "wombat", "custard", "boris",
	}
	cats = []string{
		"moss", "tabby", "whiskers", "socks", "mittens", "smudge", "tigger", "luna",
		"salem", "pumpkin", "inky", "soot", "marzipan", "purrcy", "cleo", "figaro",
	}
	birds = []string{
		"heron", "wren", "puffin", "robin", "finch", "magpie", "plover", "starling",
		"kestrel", "dodo", "lark", "pelican", "toucan", "sparrow", "gannet", "curlew",
	}
	seconds = append(append(append([]string{}, dogs...), cats...), birds...)
)

// Name is the agent's feed name, without the @. An empty id is the person
// at the keyboard.
func Name(id string) string {
	if id == "" {
		return "you"
	}
	h := fnv.New64a()
	h.Write([]byte(id))
	n := h.Sum64()
	return dogs[n%uint64(len(dogs))] + "-" + seconds[(n/uint64(len(dogs)))%uint64(len(seconds))]
}
