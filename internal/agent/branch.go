package agent

// TranscriptLocator locates saved history without promising rewind or fork.
type TranscriptLocator interface {
	TranscriptPath(p Profile, cwd, sid string) string
}

// Brancher is an agent whose conversation rush can copy for a fork that
// remembers only part of it, or that runs in another folder: the copy is
// resumed as a session of its own.
type Brancher interface {
	// TranscriptPath is where a session of p's in cwd keeps its
	// conversation.
	TranscriptPath(p Profile, cwd, sid string) string
	// Branch copies the conversation at src, up to byte upTo (-1 for all
	// of it), as session newID's in cwd, with what it needs to rewind.
	Branch(p Profile, src, cwd, sid, newID string, upTo int64) error
}

// TranscriptPath is where agent k keeps session sid's conversation in
// cwd, in profile p; empty when k can't say.
func TranscriptPath(k Kind, p Profile, cwd, sid string) string {
	if b, ok := As[TranscriptLocator](k); ok {
		return b.TranscriptPath(p, cwd, sid)
	}
	return ""
}

// FindTranscript is where session sid, last in cwd, keeps its conversation
// now: looked for, as a session that moved to a worktree keeps it where it
// started. TranscriptPath when k doesn't look. It reads the disk.
func FindTranscript(k Kind, p Profile, cwd, sid string) string {
	if t, ok := As[Transcripts](k); ok {
		return t.FindTranscript(p, cwd, sid)
	}
	return TranscriptPath(k, p, cwd, sid)
}

// ModelNamer is an agent with names of its own for its model ids.
type ModelNamer interface {
	// ModelName is id as people say it: "claude-opus-5-5" is "Opus 5.5".
	ModelName(id string) string
}

// ModelName is id as agent k says it, or id as it is.
func ModelName(k Kind, id string) string {
	if n, ok := As[ModelNamer](k); ok && id != "" {
		return n.ModelName(id)
	}
	return id
}
