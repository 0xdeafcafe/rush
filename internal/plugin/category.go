package plugin

import (
	"fmt"
	"slices"
	"strings"
)

// A Category says what a plugin is for, so Settings, Plugins and `rush
// plugin list` can put plugins that do the same kind of thing together. A
// plugin can do anything its manifest allows; its category is only where
// it's shown. One that names none is shown under Other.
type Category struct {
	ID    string // as plugin.json names it
	Title string // as rush shows it
	About string // what the plugins under it do, in a few words
}

// CategoryOther is where a plugin that names no category goes.
const CategoryOther = "other"

// Categories are every category, in the order rush shows them.
var Categories = []Category{
	{"messages", "Messages", "what you type and send: drafts, the queue"},
	{"voice", "Voice", "talk instead of typing"},
	{"agents", "Agents", "start, follow, arrange and continue agents"},
	{"code", "Code", "help agents read and change code"},
	{"pull-requests", "Pull requests", "follow and drive pull requests and their checks"},
	{"machine", "Your machine", "share its CPU and memory between agents, and clean up its disk"},
	{"views", "Views", "how rush shows what agents do"},
	{"memory", "Memory", "what agents remember across sessions"},
	{"integrations", "Other tools", "connect rush to other apps and services"},
	{CategoryOther, "Other", "plugins that name no category"},
}

// CategoryNamed is the category with id, Other if there's none.
func CategoryNamed(id string) Category {
	if i := slices.IndexFunc(Categories, func(c Category) bool { return c.ID == id }); i >= 0 {
		return Categories[i]
	}
	return Categories[len(Categories)-1]
}

// CategoryOf is the category m is shown under.
func (m Manifest) CategoryOf() Category { return CategoryNamed(m.Category) }

func (m Manifest) validateCategory() error {
	if m.Category == "" || slices.ContainsFunc(Categories, func(c Category) bool { return c.ID == m.Category }) {
		return nil
	}
	ids := make([]string, len(Categories))
	for i, c := range Categories {
		ids[i] = c.ID
	}
	return fmt.Errorf("category %q: use one of %s", m.Category, strings.Join(ids, ", "))
}
