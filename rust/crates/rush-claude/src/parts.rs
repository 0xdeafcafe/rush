//! The rest of Claude Code's adapter parts: what it offers a new session,
//! its settings, plugins, memory, stats, timeline and scratch. Port of
//! internal/adapters/claude/choices.go, branch.go, claude.go:108-150
//! (BuiltinDef, Published, Commands), scratch.go, lastquota.go,
//! summary.go, memory.go (MemoryKeeper :76-110), settings.go, settingspage.go, attribution.go,
//! plugins.go and timeline.go; each named in adapter.rs's has!.

use std::path::Path;

use rush_core::adapter::{
    self, AgentDef, Attributor, Brancher, BuiltinDefNamer, Choice, Choices, Commander, Compacter,
    Compaction, DefaultModeler, Definer, LastQuotaReader, Marketplace, Media, MediaReader, Memory,
    MemoryKeeper, MemoryNote, MemoryReader, ModelNamer, Plugger, Plugin, PluginChange, Profile,
    Published, Publisher, Scratcher, SettingsFile, SettingsFiler, SettingsPage, SettingsPager,
    SkillRooter, Stats, StatsReader, StatusLineProfiler, StatusLiner, Summarizer, TempDir,
    Timeliner,
};

use crate::account::{Account, path_str};
use crate::adapter::{Claude, KIND, account};
use rush_core::adapter::Chooser;

fn boxed(e: impl std::fmt::Display) -> adapter::Error {
    e.to_string().into()
}

fn choice(id: &str, note: &str) -> Choice {
    Choice {
        id: id.into(),
        note: note.into(),
        context: 0,
    }
}

impl Chooser for Claude {
    /// Claude Code's --model, --effort and --permission-mode.
    fn choices(&self) -> Choices {
        Choices {
            models: vec![
                choice(
                    "opus",
                    "the most capable Opus, for hard, long-running work.",
                ),
                choice(
                    "opus[1m]",
                    "Opus with the 1M-token context window, for very large tasks.",
                ),
                choice("sonnet", "faster and cheaper, good for routine work."),
                choice("haiku", "fastest and cheapest, for small tasks."),
                choice(
                    "fable",
                    "Anthropic's most capable model, at a higher price.",
                ),
            ],
            efforts: vec![
                choice(
                    "low",
                    "quick and cheap; fine for simple, well-specified tasks.",
                ),
                choice("medium", "a balance of speed and care."),
                choice(
                    "high",
                    "careful; the usual choice for real engineering work.",
                ),
                choice("xhigh", "very careful; for tricky, long-horizon tasks."),
                choice("max", "as thorough as possible, whatever it costs."),
            ],
            modes: vec![
                choice(
                    "default",
                    "asks before edits and commands it isn't sure about.",
                ),
                choice(
                    "acceptEdits",
                    "edits files without asking; still asks before other commands.",
                ),
                choice("plan", "plans first and changes nothing until you approve."),
                choice(
                    "auto",
                    "a classifier approves safe actions and asks about risky ones.",
                ),
                choice(
                    "bypassPermissions",
                    "never asks. Only for sandboxed or throwaway work.",
                ),
            ],
        }
    }
}

impl DefaultModeler for Claude {
    /// The Opus a session runs when none is picked.
    fn default_model(&self) -> String {
        "claude-opus-5-5".into()
    }
}

impl Commander for Claude {
    /// The slash commands and skills a session in cwd can run; a profile
    /// without a folder is ~/.claude.
    fn commands(&self, p: &Profile, cwd: &str) -> Vec<adapter::Command> {
        let dir = if p.dir.is_empty() {
            path_str(&Account::home().config_dir)
        } else {
            p.dir.clone()
        };
        crate::commands::commands(&dir, cwd)
    }
}

impl SkillRooter for Claude {
    /// The folders of the skills Claude Code has in cwd, less the
    /// project's own, for another agent to read as well.
    fn skill_roots(&self, p: &Profile, cwd: &str) -> Vec<String> {
        crate::commands::skill_roots(&p.dir, cwd)
    }
}

impl Brancher for Claude {
    /// The session's file in p's projects folder.
    fn transcript_path(&self, p: &Profile, cwd: &str, sid: &str) -> String {
        path_str(&account(p).transcript_path(cwd, sid))
    }
    /// Copies the transcript with the new session's id in it, then its file
    /// checkpoints, so the copy can rewind; without them it still runs.
    fn branch(
        &self,
        p: &Profile,
        src: &str,
        cwd: &str,
        sid: &str,
        new_id: &str,
        up_to: i64,
    ) -> adapter::Result<()> {
        let a = account(p);
        crate::forkcopy::copy_transcript(
            Path::new(src),
            &a.transcript_path(cwd, new_id),
            sid,
            new_id,
            up_to,
        )?;
        a.copy_checkpoints(sid, new_id)?;
        Ok(())
    }
}

impl ModelNamer for Claude {
    /// Claude's model id as people say it: "Opus 5.5".
    fn model_name(&self, id: &str) -> String {
        crate::modelname::model_name(id)
    }
}

impl Attributor for Claude {
    fn attribution(&self, p: &Profile) -> (bool, bool) {
        crate::settings::attribution_in(&p.dir)
    }
    fn set_attribution(&self, p: &Profile, on: bool) -> adapter::Result<()> {
        Ok(crate::settings::set_attribution_in(&p.dir, on)?)
    }
}

impl MediaReader for Claude {
    /// What every current Claude model reads: images and PDFs, not audio or
    /// video.
    fn reads(&self, _model: &str) -> Option<Media> {
        Some(Media::IMAGE | Media::PDF)
    }
}

impl Compacter for Claude {
    fn compaction(&self, p: &Profile, cwd: &str, env: &[String]) -> Compaction {
        crate::settings::compaction_for(p, cwd, env)
    }
}

impl Publisher for Claude {
    /// @anthropic-ai/claude-code, which updates itself with claude update.
    fn published(&self) -> Published {
        Published {
            npm: "@anthropic-ai/claude-code".into(),
            pypi: String::new(),
            update: vec!["update".into()],
        }
    }
}

impl LastQuotaReader for Claude {
    fn last_quota(&self, p: &Profile) -> Option<rush_core::usage::Quota> {
        self.last_quota_of(p)
    }
}

impl Summarizer for Claude {
    /// Claude's, the cheapest first.
    fn summary_models(&self) -> adapter::Result<Vec<String>> {
        Ok(crate::summary::summary_models())
    }
    /// claude -p with no tools, as the login in use.
    fn summarize(&self, model: &str, system: &str, text: &str) -> adapter::Result<String> {
        let prog = adapter::global().path(KIND);
        crate::summary::summarize(&prog, &self.run_as().env(), model, system, text).map_err(boxed)
    }
}

impl StatsReader for Claude {
    /// The profile's stats-cache.json, what Claude Code's /stats shows.
    fn stats(&self, p: &Profile) -> adapter::Result<Stats> {
        Ok(crate::stats::load_stats(&account(p))?)
    }
}

impl SettingsFiler for Claude {
    fn settings_files(&self, p: &Profile, cwd: &str) -> Vec<SettingsFile> {
        crate::settings::settings_files(p, cwd)
    }
}

impl StatusLiner for Claude {
    fn status_line(&self, p: &Profile) -> adapter::Result<String> {
        Ok(crate::settings::status_line(p)?)
    }
    fn set_status_line(&self, p: &Profile, command: &str) -> adapter::Result<()> {
        Ok(crate::settings::set_status_line(p, command)?)
    }
}

impl StatusLineProfiler for Claude {
    fn status_line_profile(&self) -> Profile {
        crate::settings::status_line_profile()
    }
}

impl SettingsPager for Claude {
    fn settings_page(&self) -> SettingsPage {
        crate::settings::settings_page()
    }
}

impl BuiltinDefNamer for Claude {
    /// Claude Code's own agent, which runs when no definition is picked.
    fn builtin_def(&self) -> String {
        "claude".into()
    }
}

impl Definer for Claude {
    fn agent_defs(&self, p: &Profile, roots: &[String]) -> Vec<AgentDef> {
        crate::settings::agent_defs(p, roots)
    }
    fn new_agent_def(&self, p: &Profile, name: &str) -> adapter::Result<String> {
        Ok(crate::settings::new_agent_def(p, name)?)
    }
}

impl Plugger for Claude {
    fn plugins(&self, p: &Profile, dir: &str) -> adapter::Result<(Vec<Plugin>, Vec<Plugin>)> {
        Ok(crate::plugins::plugins(&account(p), dir)?)
    }
    fn marketplaces(&self, p: &Profile, dir: &str) -> adapter::Result<Vec<Marketplace>> {
        Ok(crate::plugins::marketplaces(&account(p), dir)?)
    }
    fn plugin_cost(&self, p: &Profile, dir: &str, id: &str) -> adapter::Result<String> {
        Ok(crate::plugins::plugin_cost(&account(p), dir, id)?)
    }
    fn change_plugins(&self, p: &Profile, dir: &str, c: &PluginChange) -> adapter::Result<()> {
        Ok(crate::plugins::change_plugins(&account(p), dir, c)?)
    }
}

impl MemoryReader for Claude {
    /// What a session of p's in cwd reads. Its auto memory is the
    /// repository's, kept under its main checkout's folder in p's projects,
    /// so every worktree of it shares one; out of git, the folder's own,
    /// beside its transcript.
    fn memory(&self, p: &Profile, cwd: &str, transcript: &str) -> Memory {
        crate::memory::memory(&p.dir, cwd, transcript)
    }
    fn forget_file(&self, path: &str) -> adapter::Result<()> {
        Ok(crate::memory::forget_file(path)?)
    }
}

impl MemoryKeeper for Claude {
    /// Writes a memory note for p's sessions in cwd.
    fn remember(&self, p: &Profile, cwd: &str, n: &MemoryNote) -> adapter::Result<String> {
        let proj = crate::memory::project_folder(&p.dir, cwd);
        if proj.is_empty() {
            return Err("no project to remember it in".into());
        }
        let n = crate::notes::Note {
            name: n.name.clone(),
            title: n.title.clone(),
            description: n.description.clone(),
            kind: n.kind.clone(),
            body: n.body.clone(),
        };
        Ok(crate::notes::write_note(
            &crate::files::join(&[&proj, "memory"]),
            n,
        )?)
    }
    /// Adds MEMORY.md lines for notes without one, and takes out those for
    /// notes that don't exist.
    fn tidy_memory(&self, dir: &str) -> adapter::Result<Vec<String>> {
        Ok(crate::notes::fix_memory(dir)?)
    }
    /// Puts drop's text below keep's, and deletes drop.
    fn merge_notes(&self, keep: &str, drop: &str) -> adapter::Result<()> {
        Ok(crate::notes::merge_notes(keep, drop)?)
    }
    /// Moves a note into p's own CLAUDE.md, which every project loads.
    fn make_global(&self, p: &Profile, path: &str) -> adapter::Result<()> {
        Ok(crate::notes::make_global(&p.dir, path)?)
    }
    fn read_note(&self, path: &str) -> adapter::Result<MemoryNote> {
        let n = crate::notes::read_note(path)?;
        Ok(MemoryNote {
            name: n.name,
            title: n.title,
            description: n.description,
            kind: n.kind,
            body: n.body,
        })
    }
    fn count_opens(&self, _: &Profile, mem: &mut Memory) {
        crate::memory::count_opens(mem);
    }
}

impl Timeliner for Claude {
    /// Follows a Claude Code transcript and its subagents'.
    fn new_timeline(&self) -> Box<dyn adapter::Timeline> {
        Box::new(crate::timeline::Timeline::new())
    }
}

impl Scratcher for Claude {
    /// A background job's own tmp folder, and every session's scratch
    /// (task output and the like) under /tmp/claude-<uid>.
    fn scratch(&self, p: &Profile, job: &str, sid: &str, cwd: &str) -> Vec<TempDir> {
        let mut out = Vec::new();
        if !job.is_empty() {
            out.push(TempDir {
                path: path_str(&account(p).jobs_dir().join(job).join("tmp")),
                keep: true,
            });
        }
        if !sid.is_empty() && !cwd.is_empty() {
            out.push(TempDir {
                path: path_str(
                    &Path::new(&self.scratch_root())
                        .join(crate::jobs::project_slug(cwd))
                        .join(sid),
                ),
                keep: false,
            });
        }
        out
    }
    /// Where Claude Code keeps per-session scratch.
    fn scratch_root(&self) -> String {
        // SAFETY: getuid can't fail.
        format!("/tmp/claude-{}", unsafe { libc::getuid() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rush_core::adapter::Adapter;

    // The adapter names each part it has: a few, through the registry's
    // own accessors, as rush asks them.
    #[test]
    fn parts_are_named() {
        let dir = tempfile::tempdir().unwrap();
        let c = Claude::at(Account::at("", dir.path()));
        assert_eq!(c.chooser().unwrap().choices().models[0].id, "opus");
        assert_eq!(
            c.model_namer().unwrap().model_name("claude-opus-5-5[1m]"),
            "Opus 5.5"
        );
        assert_eq!(
            c.default_modeler().unwrap().default_model(),
            "claude-opus-5-5"
        );
        assert_eq!(c.builtin_def_namer().unwrap().builtin_def(), "claude");
        assert!(
            c.media_reader()
                .unwrap()
                .reads("x")
                .unwrap()
                .has(Media::PDF)
        );
        assert!(c.timeliner().is_some() && c.memory_reader().is_some() && c.plugger().is_some());
        assert!(c.accounts().is_some() && c.plan_reader().is_some() && c.quota_source().is_some());
        assert!(c.run_follower().unwrap().subagent_runs().is_some());
        let p = crate::adapter::profile(&Account::at("", dir.path()));
        let s = c.scratcher().unwrap().scratch(&p, "abcd1234", "s1", "/w");
        assert_eq!(s.len(), 2);
        assert!(s[0].keep && s[0].path.ends_with("jobs/abcd1234/tmp"));
        assert!(s[1].path.ends_with("/-w/s1") && !s[1].keep);
        assert_eq!(
            c.publisher().unwrap().published().npm,
            "@anthropic-ai/claude-code"
        );
    }

    #[test]
    fn branch_copies_and_keeps_checkpoints() {
        let dir = tempfile::tempdir().unwrap();
        let a = Account::at("", dir.path());
        let c = Claude::at(a.clone());
        let p = crate::adapter::profile(&a);
        let src = a.transcript_path("/w", "old");
        std::fs::create_dir_all(src.parent().unwrap()).unwrap();
        std::fs::write(&src, "{\"sessionId\":\"old\"}\n").unwrap();
        std::fs::create_dir_all(a.config_dir.join("file-history/old")).unwrap();
        std::fs::write(a.config_dir.join("file-history/old/f@v1"), "x").unwrap();
        c.brancher()
            .unwrap()
            .branch(&p, &src.to_string_lossy(), "/w", "old", "new", -1)
            .unwrap();
        let got = std::fs::read_to_string(a.transcript_path("/w", "new")).unwrap();
        assert_eq!(got, "{\"sessionId\":\"new\"}\n");
        assert!(a.config_dir.join("file-history/new/f@v1").exists());
    }
}
