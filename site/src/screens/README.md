# Captured terminal frames

These `.ansi` files are unedited `tmux capture-pane -p -e` output from the
installed rush, not authored terminal markup. Each adjacent JSON file records
its terminal width/height, installed rush version and capture time.

The world is fictional: Sam's repositories, transcripts, sample chirps and
simulated deploy commands. `tools/shots` puts it under a separate temporary
home and runs the installed rush. The Agents fixtures use inert copies of the
capture tool named `claude`, blocked on stdin, so rush recognises live workers.
No model is called. The before capture has two working jobs; the after capture
has the tax-test job blocked on its actual fixture question.

From the repository root, regenerate with:

```sh
go -C tools/shots run . -only agents -ansi-only -ansi-out ../../site/src/screens -root "$TMPDIR/site-shots-world"
go -C tools/shots run . -only feed -ansi-only -ansi-out ../../site/src/screens -root "$TMPDIR/site-shots-world"
go -C tools/shots run . -only command-run -ansi-only -ansi-out ../../site/src/screens -root "$TMPDIR/site-shots-world"
```

`-only agents` includes `agents-before` and `subagents`. The fictional world is
rebuilt for each shot. Times, process measurements and terminal status can vary.
Review the captures after regeneration; the page intentionally crops specific
rows/columns and expects the question and feed to be present.

`AnsiScreen.tsx` parses at build time and renders the captured terminal cells.
The site has no terminal bitmap, canvas terminal or client-side ANSI parser.
The Agents animation matches rows between the two frames; it does not invent a
question or manually redraw a row. Captions disclose the fictional inputs.
