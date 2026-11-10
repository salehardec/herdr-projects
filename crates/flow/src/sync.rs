//! Keeping a task's branch current with main, in the task's own worktree. The board does this by itself, in code:
//! before a finished turn is checked and reviewed, again whenever main moves while the task waits for you (under
//! `merge.sync approve` only a test merge then, sync/probe.rs), and one last time after you approve (land/). The agent
//! hears about it only when a real conflict exists. A merge attempt that fails is always rolled back, and the human's
//! checkout is never touched. Main is the branch the task will land on (sync/target.rs): the branch it started from by
//! name, `merge.into`, or origin's own default branch — never the branch your checkout happens to be on. Only in a repo
//! with no remote at all is it the branch your checkout is on, the one case the board merges into it.
pub mod conflict;
pub mod conflict_log;
pub mod merge_only;
pub mod probe;
pub mod target;
pub use conflict::{conflict_back, conflict_failure, conflict_message};
pub use merge_only::merge_only;
use crate::flow::git::{has_origin, head_of};
use crate::json::*;
use crate::store::fetch;
use crate::store::repos::{branch_of, folder_of, git_out};
use crate::store::tasks::Task;
use std::collections::HashMap;
use std::path::Path;

fn lines(text: &str) -> Vec<String> { text.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect() }
/// The branch a task's work lands on (sync/target.rs), for the checks that only want its name. Empty when nothing names
/// it: a sync or a merge then fails, saying what to set.
pub fn main_branch(root: &Path, t: &Obj) -> String { target::of(root, t).map(|x| x.branch).unwrap_or_default() }
/// The branch a task started from by name (`workspace.from develop`) and lands on, when it is not origin's main.
pub fn named_base(root: &Path, t: &Obj) -> Option<String> {
    let b = t.s("from").strip_prefix("origin/").filter(|b| *b != "HEAD")?;
    (target::known_default(root) != b).then(|| b.to_string())
}
/// The branch a task lands on, for the words that name it: its start's named branch, `merge.into`, or "main".
pub fn main_word(t: &Obj) -> String {
    let b = main_branch(&folder_of(t.s("id")), t);
    if b.is_empty() { "main".into() } else { b }
}
/// The branch a task's work lands on, in words, for the question that asks before a merge.
pub fn lands_on(t: &Obj) -> String {
    let root = folder_of(t.s("id"));
    let b = if has_origin(&root) { target::of(&root, t).map(|x| x.branch).unwrap_or_else(|_| "no branch named".into()) } else { branch_of(&root) };
    if b.is_empty() { "main".into() } else { b }
}
/// The task's own worktree: the only place a sync ever merges. Empty for a task that works in the human's checkout.
pub fn tree_of(t: &Obj) -> String {
    let root = folder_of(t.s("id"));
    let wt = t.s("worktree");
    if !wt.is_empty() && Path::new(wt).exists() && crate::store::files::absolute(Path::new(wt)) != crate::store::files::absolute(&root) { wt.to_string() } else { String::new() }
}
/// A task the board may re-sync by itself when main moves: at rest, waiting for your approval, nothing else blocking.
pub fn resyncable(t: &Obj) -> bool { t.s("stage") == "approval" && !t.b("archivedAt") && !t.b("mergeIntent") && !t.b("autoMerge") && !t.b("failure") && crate::flow::task::is_agent_task(t) }
/// Where main is known to stand, without asking the remote: cheap enough for every housekeeping pass.
pub fn known_target(t: &Obj, cache: &mut HashMap<String, String>) -> String {
    let root = folder_of(t.s("id"));
    let b = if has_origin(&root) { target::known(&root, t) } else { String::new() };
    let key = format!("{}|{b}", root.display());
    if let Some(v) = cache.get(&key) { return v.clone(); }
    let sha = if has_origin(&root) {
        if b.is_empty() { String::new() } else { git_out(&root, &["rev-parse", "--verify", &format!("refs/remotes/origin/{b}")]).unwrap_or_default() }
    } else { let h = head_of(&root); let h = if h.is_empty() { git_out(&root, &["rev-parse", "HEAD"]).unwrap_or_default() } else { h }; if h.is_empty() { String::new() } else { format!("{h} {}", branch_of(&root)) } };
    cache.insert(key, sha.clone());
    sha
}
/// Ask the remote where main is. The remote-tracking ref is named, so what git knows locally is updated by every fetch;
/// the tasks of a repo fetch it one at a time, and share a fetch (store/fetch.rs).
pub fn fetch_main(dir: &Path, b: &str) -> crate::err::Res<()> { fetch::fetch_branch(dir, b) }
/// Where main stands at the remote, asked now: the remote-tracking ref of `b`, read after the fetch (never FETCH_HEAD,
/// which a fetch shared with another task does not write). A fetch that lost a race for the ref to another one (an
/// agent's own, yours) takes what that one wrote. Any other failure says nothing about main: Err says why in plain
/// words, then what git said.
fn remote_main(tree: &Path, b: &str) -> Result<String, String> {
    let fetched = fetch_main(tree, b);
    let known = git_out(tree, &["rev-parse", "--verify", "-q", &format!("refs/remotes/origin/{b}^{{commit}}")]);
    match (fetched, known) {
        (Ok(()), Ok(x)) => Ok(x),
        (Err(e), Ok(x)) if fetch::moved(&e.msg) => Ok(x),
        (Err(e), _) => Err(e.msg),
        (Ok(()), Err(e)) => Err(format!("origin/{b} could not be read after the fetch.\n\nWhat git said:\n{}", e.msg)),
    }
}
/// What a sync found: current, merged, conflict, skip, no-target or error (and clean, for a test merge), with what it
/// looked at. `landing` is the branch at origin the work lands on and `onto_from` what named it (sync/target.rs), for
/// the task's record; `onto` is the branch your checkout is on, only for a repo with no remote at all.
#[derive(Clone, Debug, Default)]
pub struct Synced { pub status: &'static str, pub target: String, pub seen: String, pub base: String, pub landing: String, pub onto: Option<String>, pub onto_from: String, pub adopt: bool, pub head: String, pub before: String, pub overlap: Option<Vec<String>>, pub files: String, pub restored: bool, pub out: String, pub want: String, pub detail: String }
/// Fetch main and merge it into the task's branch. Every result that looked at main carries `seen`: where main stood in
/// the form known_target gives it, for exactly the commit that was merged here.
pub fn sync_main(t: &Task, tree: &str) -> Synced { look(t, tree, false) }
/// The same, merged only in memory (sync/probe.rs): nothing in the worktree, the branch or at the remote changes. The
/// task's own branch is what is test-merged, wherever its worktree stands.
pub fn test_main(t: &Task, tree: &str) -> Synced { look(t, tree, true) }
fn look(t: &Task, tree: &str, test: bool) -> Synced {
    let root = folder_of(t.s("id"));
    let tree = Path::new(tree);
    let (target, base, landing, onto, adopt, onto_from);
    if has_origin(&root) {
        // The branch the work lands on is a branch at origin (sync/target.rs): one its start named, `merge.into`, or
        // origin's own default branch. It is fetched, and the work keeps up with its copy at origin. The branch your
        // checkout is on is never read for it. Nothing names it: Err, and the task fails rather than guesses.
        let tg = match target::of(&root, t) { Ok(x) => x, Err(detail) => return Synced { status: "no-target", detail, ..Default::default() } };
        target = match remote_main(tree, &tg.branch) {
            Ok(x) => x,
            Err(detail) => return Synced { status: "error", detail, want: tg.branch.clone(), landing: tg.branch, onto_from: tg.from, ..Default::default() },
        };
        base = format!("origin/{}", tg.branch); landing = tg.branch; onto = None; adopt = false; onto_from = tg.from;
    } else {
        // No origin at all: there is nowhere to push, and the branch your checkout is on is the only main there is. It
        // must be on one, and it is read, never switched (land/merge.rs merges there).
        let on = branch_of(&root);
        let mut ad = false;
        if t.b("syncBranch") && on != t.s("syncBranch") {
            let gone = git_out(&root, &["rev-parse", "--verify", "-q", &format!("refs/heads/{}", t.s("syncBranch"))]).is_err();
            if !gone { return Synced { status: "skip", onto: Some(on), want: t.t("syncBranch"), ..Default::default() }; }
            ad = true;
        }
        let h = head_of(&root);
        target = if h.is_empty() { match git_out(&root, &["rev-parse", "HEAD"]) { Ok(x) => x, Err(e) => return Synced { status: "error", detail: e.msg, ..Default::default() } } } else { h };
        base = if on.is_empty() { "HEAD".into() } else { on.clone() };
        landing = String::new(); onto = Some(on); adopt = ad; onto_from = target::CHECKOUT.into();
    }
    let seen = match &onto { None => target.clone(), Some(o) => format!("{target} {o}") };
    let tip = if test && !t.s("branch").is_empty() { format!("refs/heads/{}", t.s("branch")) } else { "HEAD".into() };
    let before = if test { git_out(tree, &["rev-parse", "--verify", "-q", &tip]).unwrap_or_default() } else { head_of(tree) };
    let r = Synced { target: target.clone(), seen, base, landing, onto, onto_from, adopt, ..Default::default() };
    if git_out(tree, &["merge-base", "--is-ancestor", &target, &tip]).is_ok() { return Synced { status: "current", head: before, ..r }; }
    if test { return probe::test_merge(tree, &tip, Synced { before, ..r }); }
    if let Err(e) = git_out(tree, &["merge", "--no-edit", &target]) {
        let files = git_out(tree, &["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default();
        // Rolled back means: no merge is in progress any more and the branch is where it was.
        let _ = git_out(tree, &["merge", "--abort"]);
        let restored = head_of(tree) == before && git_out(tree, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]).is_err();
        return Synced { status: "conflict", files, restored, before, out: e.msg, ..r };
    }
    let head = head_of(tree);
    // What the update changed here, against what the task itself changed. Unknown counts as overlapping.
    let overlap = (|| -> crate::err::Res<Vec<String>> {
        let theirs: std::collections::HashSet<String> = lines(&git_out(tree, &["diff", "--name-only", "--no-renames", &before, &head])?).into_iter().collect();
        Ok(lines(&git_out(tree, &["diff", "--name-only", "--no-renames", &format!("{target}...{before}")])?).into_iter().filter(|f| theirs.contains(f)).collect())
    })().ok();
    Synced { status: "merged", before, head, overlap, ..r }
}
fn retry_ms() -> i64 { std::env::var("HERDR_PROJECTS_SYNC_RETRY_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(60000) }
pub fn skip_why(r: &Synced) -> String { format!("your checkout is on {}, not on {}, the branch this task lands on", r.onto.as_deref().filter(|s| !s.is_empty()).unwrap_or("no branch"), r.want) }
/// Why work that was checked is not up to date with where it lands, so it cannot be approved yet: empty when it is.
/// Main that could not be fetched is said in plain words; the board tries again by itself.
pub fn held_why(r: &Synced) -> String {
    match r.status {
        "skip" => skip_why(r),
        "no-target" => r.detail.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("nothing says where this work lands").chars().take(300).collect(),
        "error" => {
            let f: String = r.detail.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").chars().take(300).collect();
            if f.starts_with("main could not be test-merged") { return f.trim_end_matches('.').into(); }
            format!("main could not be fetched ({})", if f.is_empty() { "git fetch failed" } else { f.trim_end_matches('.') })
        }
        _ => String::new(),
    }
}
/// What a task held as "behind main" keeps: why, in plain words, since when, and what git itself said, for its details.
pub fn blocked(why: &str, r: Option<&Synced>) -> Value {
    let detail = r.filter(|r| r.status == "error").map(|r| r.detail.clone()).unwrap_or_default();
    json!({ "why": why, "at": crate::clock::now(), "detail": if detail.is_empty() { Value::Null } else { json!(detail) } })
}
pub fn behind_msg(why: &str) -> String { format!("Behind main, and not up to date yet: {why}. The board keeps trying; it cannot be approved until then") }
/// What a task remembers of a sync that was tried: where main stood for the commit that was really merged. Only a
/// sync that really looked counts as seen; one that could not read main or was not tried marks only when to try again.
pub fn seen_patch(t: &Obj, r: Option<&Synced>) -> Obj {
    let Some(r) = r.filter(|r| r.status != "error" && r.status != "skip" && r.status != "no-target") else { return obj(json!({ "syncRetryAt": crate::clock::now() + retry_ms() })) };
    let mut p = obj(json!({ "syncSeen": r.seen, "syncRetryAt": 0, "syncBlocked": null }));
    if !r.target.is_empty() && (r.status == "current" || r.status == "merged") { p.put("syncedTo", r.target.clone()); p.put("syncRepairs", 0); }
    if let Some(o) = &r.onto { if !o.is_empty() && (!t.b("syncBranch") || r.adopt) { p.put("syncBranch", o.clone()); } }
    // Where a task with a remote lands, and what named it (`origin/HEAD`, `merge.into`, `workspace.from`), so a wrong
    // target is seen at once in task.json and in the merge's message. The branch your checkout is on is never it.
    if !r.landing.is_empty() {
        p.put("syncBranch", r.landing.clone()); p.put("mergeInto", r.landing.clone()); p.put("mergeIntoFrom", r.onto_from.clone());
    }
    p
}
pub fn apply(t: &mut Obj, p: Obj) { for (k, v) in p { t.insert(k, v); } }
