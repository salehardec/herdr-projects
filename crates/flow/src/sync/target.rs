//! Where a task's work lands, and where that answer came from: the branch it started from by name (`workspace.from
//! develop`), the branch `merge.into` names, or origin's own default branch (`refs/remotes/origin/HEAD`, asked of
//! origin once when it was never set here). Never the branch the human's checkout happens to be on. When none of them
//! names one the task fails and says so: the board does not guess.
use crate::json::*;
use crate::store::fetch;
use crate::store::repos::git_out;
use crate::store::settings::lookup::setting_text;
use std::path::Path;

/// The branch a task's start named, and where its work lands.
pub const FROM: &str = "workspace.from";
/// The setting that names a landing branch for tasks that do not name one.
pub const SETTING: &str = "merge.into";
/// Origin's own default branch, the one `refs/remotes/origin/HEAD` points at.
pub const ORIGIN_HEAD: &str = "origin/HEAD";
/// The branch a repo with no remote at all merges into: the one the human's checkout is on.
pub const CHECKOUT: &str = "checkout";

/// The branch a task lands on, and which of the constants above named it.
#[derive(Clone, Debug, Default)]
pub struct Target { pub branch: String, pub from: String }

/// Origin's own default branch as this repo already knows it (`refs/remotes/origin/HEAD`), never asking origin. Empty
/// when it is not known here.
pub fn known_default(root: &Path) -> String {
    git_out(root, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]).ok().and_then(|r| r.trim().strip_prefix("origin/").filter(|b| !b.is_empty()).map(str::to_string)).unwrap_or_default()
}
/// Origin's own default branch: `refs/remotes/origin/HEAD`, asked of origin once and kept when it was never set here.
/// Err: origin did not say which branch is its own, and the board does not guess main or master.
pub fn default_branch(root: &Path) -> Result<String, String> {
    let said = "origin's default branch is not known here: refs/remotes/origin/HEAD is not set, and origin did not say which branch is its own (it may be out of reach, or have no HEAD)";
    if !fetch::learn_origin_head(root) { return Err(said.into()); }
    let b = known_default(root);
    if b.is_empty() { Err(said.into()) } else { Ok(b) }
}
/// The branch a task names itself: the one its start named (`workspace.from develop`), else `merge.into`. None: it
/// names none, and origin's default branch is the answer.
fn named(t: &Obj) -> Option<(String, &'static str)> {
    if let Some(b) = t.s("from").strip_prefix("origin/").filter(|b| *b != "HEAD") { return Some((b.to_string(), FROM)); }
    let into = setting_text(SETTING, Some(t));
    (!into.is_empty() && into != "none").then(|| (into, SETTING))
}

/// Where a task lands, as it is known now and without asking origin. Empty when nothing names it yet. For the cheap
/// passes (housekeeping's `known_target`), which run for every waiting task and must never stop on a remote that does
/// not answer.
pub fn known(root: &Path, t: &Obj) -> String { named(t).map(|(b, _)| b).unwrap_or_else(|| known_default(root)) }

/// Where a task's work lands: a branch its start named, else `merge.into`, else origin's default branch. Err: what to
/// change, in plain words. A branch your checkout is on is never the answer.
pub fn of(root: &Path, t: &Obj) -> Result<Target, String> {
    match named(t) {
        Some((branch, from)) => Ok(Target { branch, from: from.into() }),
        None => Ok(Target { branch: default_branch(root)?, from: ORIGIN_HEAD.into() }),
    }
}

/// The failure a task gets when nothing names where its work lands: its title, the details, and what to do. Nothing was
/// pushed, and the branch your checkout is on is not used for it.
pub fn nowhere(detail: &str, t: &Obj) -> (String, String, String) {
    let repo = t.s("repo");
    ( "Nothing says where this work lands".into(),
      detail.to_string(),
      format!("Name the branch the work belongs to (herdr-projects set merge.into main{}), or make origin's default branch known (git fetch origin, then git remote set-head origin --auto), then finish again. Nothing was pushed, and the branch your checkout is on was not used.", if repo.is_empty() { String::new() } else { format!(" --repo {repo}") }) )
}
