//! An agent that needs you to decide something ends its turn with a
//! decisions block (read by rush_convo::decisions). The host shows it as a
//! card (Info.decisions) and your answers go back as the next message
//! (decision_answer). It's read at every turn's end.
//!
//! Ported from internal/host/decisions.go:36-50 (decisionsPrompt,
//! decisionsMost), 377-463 (DecisionsOf, DecisionAnswer, DismissAsleep,
//! NeedsDecisions).

use std::collections::HashMap;

use rush_core::event::{Ask, Choice, Question};

/// How many decisions a card takes.
pub const DECISIONS_MOST: usize = 9;

/// Tells the agent how to ask for decisions so rush can draw them as a
/// card: kept short, as every session carries it.
pub const DECISIONS_PROMPT: &str = "When you need the user to decide something, end your reply with a decisions block, which rush shows as a card:
Decisions:
1. **Topic:** What's at stake and why it's theirs to decide, in a sentence or two they can follow without the rest of your reply. (a) option: what it does, and what it costs; (b) option: ….
   (a) option:
   ```
   what they'd get with it: a mockup of the screen, the code, the output, the message
   ```
My picks: 1a 2b.
One numbered item per decision, a yes/no as (a) yes; (b) no. The topic is the card's title: a few plain words. When the options differ in what the user would see or get, give each one a preview like (a)'s, so they pick by looking, not by imagining it; the card shows it beside the option. Answers come back as `1a. Topic: option` or `2: their words`.";

/// n in base 36, as Go's strconv.FormatInt(n, 36).
fn base36(mut n: i128) -> String {
    if n == 0 {
        return "0".into();
    }
    let neg = n < 0;
    let mut out = Vec::new();
    while n != 0 {
        let d = (n % 36).unsigned_abs() as u32;
        out.push(std::char::from_digit(d, 36).unwrap_or('0'));
        n /= 36;
    }
    if neg {
        out.push('-');
    }
    out.iter().rev().collect()
}

/// text's decisions block as a question card, None when it has none: each
/// decision an ask keyed by its number, each option a choice keyed by its
/// letter, the picks marked.
pub fn decisions_of(text: &str) -> Option<Question> {
    let (_, ds, _) = rush_convo::decisions::decisions_in(text);
    if ds.is_empty() {
        return None;
    }
    let mut q = Question {
        id: format!(
            "decisions-{}",
            base36(rush_state::Time::now().unix_nanos())
        ),
        ..Question::default()
    };
    for d in ds.iter().take(DECISIONS_MOST) {
        let mut a = Ask {
            id: d.r#ref.clone(),
            header: d.topic.clone(),
            text: format!("{}. {}", d.r#ref, d.topic),
            ..Ask::default()
        };
        if !d.context.is_empty() {
            a.text = format!("{}\n\n{}", d.context, a.text); // the card's lead-in, then the ask
        }
        for o in &d.options {
            a.options.push(Choice {
                key: o.key.clone(),
                label: o.label.clone(),
                description: o.detail.clone(),
                preview: o.preview.clone(),
                picked: o.recommended,
            });
        }
        q.asks.push(a);
    }
    Some(q)
}

/// The message that answers q, in its own numbering, one paragraph a
/// decision: "1a. Topic: label" for an option, "2: text" for words of
/// your own. answers are by ask text, as the question card keeps them; one
/// left unanswered is left out.
pub fn decision_answer(q: &Question, answers: &HashMap<String, String>) -> String {
    let mut out = Vec::new();
    for a in &q.asks {
        let ans = answers.get(&a.text).map(|s| s.trim()).unwrap_or("");
        if ans.is_empty() {
            continue;
        }
        let mut line = format!("{}: {ans}", a.id);
        if let Some(o) = a.options.iter().find(|o| o.label == ans) {
            line = format!("{}{}. {}: {ans}", a.id, o.key, a.header);
        }
        out.push(line);
    }
    out.join("\n\n")
}

/// Sets decisions card dec_id aside on a session whose host is asleep,
/// without waking it.
pub fn dismiss_asleep(id: &str, dec_id: &str) -> std::io::Result<()> {
    std::fs::write(
        crate::paths::dir(id).join(crate::client::DECISIONS_GONE),
        dec_id,
    )
}

/// What the list says a session with decisions open needs of you.
pub fn needs_decisions(q: &Question) -> String {
    if q.asks.len() == 1 {
        return format!("decide: {}", q.asks[0].header);
    }
    format!("{} decisions", q.asks.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    // decisions_test.go:19
    pub(crate) const M1326: &str = "**Branch prompts in boxer's project context:** I read boxer's root (#1) from here.\n\n**My take:** you're right, they don't belong there in full.\n- **Already covered:** each branch's abstract already opens with an \"Ask:\" line.\n\nDecisions (my picks: 1a 2a):\n1. **Branch starts in the context:** (a) one-line intent plus breadcrumb; (b) drop them, rely on abstracts; (c) leave as is.\n2. **Boxer's copied `agents/ultimate.md`:** (a) assess a way for projects to add tools to @ultimate without forking it; (b) leave it.";

    // decisions_test.go:139
    #[test]
    fn decision_card() {
        let q = decisions_of(M1326).expect("a card");
        assert_eq!(q.asks.len(), 2);
        assert_eq!(q.asks[0].id, "1");
        assert_eq!(q.asks[0].header, "Branch starts in the context");
        let o = &q.asks[0].options;
        assert!(o.len() == 3 && o[0].picked && !o[1].picked && o[2].key == "c", "{o:?}");
        // In the message's own numbering, typed text as "n: text", the
        // unanswered left out.
        let got = decision_answer(
            &q,
            &HashMap::from([
                (q.asks[0].text.clone(), "one-line intent plus breadcrumb".into()),
                (q.asks[1].text.clone(), "only for @ultimate, not reviewers".into()),
            ]),
        );
        assert_eq!(
            got,
            "1a. Branch starts in the context: one-line intent plus breadcrumb\n\n2: only for @ultimate, not reviewers"
        );
        let got = decision_answer(&q, &HashMap::from([(q.asks[1].text.clone(), "leave it".into())]));
        assert_eq!(got, "2b. Boxer's copied agents/ultimate.md: leave it");
        let mut many = String::new();
        for i in 1..=12 {
            many += &format!("{i}. **T{i}:** (a) yes; (b) no.\n");
        }
        assert_eq!(decisions_of(&many).unwrap().asks.len(), DECISIONS_MOST);
        assert_eq!(needs_decisions(&q), "2 decisions");
        assert!(q.id.starts_with("decisions-"));
    }

    #[test]
    fn base36_as_go() {
        assert_eq!(base36(0), "0");
        assert_eq!(base36(35), "z");
        assert_eq!(base36(36), "10");
        assert_eq!(base36(1_700_000_000_000_000_000), "cwyvpelgpse8");
    }

    // decisions_test.go:473: the prompt's own example is a block rush
    // reads, its preview on (a).
    #[test]
    fn prompt_example_reads() {
        let (_, ds, _) = rush_convo::decisions::decisions_in(DECISIONS_PROMPT);
        assert!(
            ds.len() == 1
                && ds[0].options.len() == 2
                && ds[0].options[0].preview.starts_with("what they'd get")
                && ds[0].options[1].preview.is_empty(),
            "{ds:?}"
        );
    }
}
