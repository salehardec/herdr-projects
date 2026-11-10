//! Merging an approved task with no pull request. In a repo with a remote, the work lands on the branch `merge.into`
//! names, else origin's own default branch — never on the branch your checkout is on — pushed there from the task's own
//! worktree and never merged into your checkout, whatever is unsaved or checked out there. Only what origin shows after
//! the push is reported as merged: a push origin refuses, and one git said went but origin does not show, fail the task
//! and keep its worktree. Nothing naming a landing branch fails the task with what to set, and pushes nothing. With no
//! remote at all the work is merged into your checkout, squashed as it always was, and unsaved files there stop it and
//! nothing is forced.
use crate::common;
use common::*;
use serde_json::json;
use std::path::{Path, PathBuf};
use sw::actions::finish_line;
use sw::json::*;

fn push_world(more: Value) -> World {
    let mut s = json!({ "review.when": "never", "merge.pr": false });
    for (k, v) in more.as_object().cloned().unwrap_or_default() { s[k] = v; }
    world_with(Opts { repo: true, origin: true, settings: s, ..Default::default() })
}
fn bare(w: &World) -> PathBuf { w.base.join("origin.git") }
fn origin(w: &World, b: &str) -> String { git(&bare(w), &["rev-parse", &format!("refs/heads/{b}")]) }
fn has(w: &World, b: &str, file: &str) -> Option<String> { common::worktrees::git(&bare(w), &["show", &format!("{b}:{file}")]).ok() }
fn hook(dir: &Path, name: &str, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    let f = dir.join("hooks").join(name);
    std::fs::write(&f, format!("#!/bin/sh\n{body}")).unwrap();
    std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
}
/// Someone else pushes a commit to main at the remote: what they pushed.
fn they_push(w: &World, file: &str, body: &str) -> String {
    let other = w.base.join("other");
    git(&w.base, &["clone", "-q", &bare(w).display().to_string(), &other.display().to_string()]);
    std::fs::write(other.join(file), body).unwrap(); git(&other, &["add", "-A"]); git(&other, &["commit", "-qm", "theirs"]); git(&other, &["push", "-q", "origin", "main"]);
    git(&other, &["rev-parse", "HEAD"])
}
fn approved(id: &str) -> Obj { finish_line::approve(id, None, false).unwrap(); settle(id) }

#[test]
fn unsaved_files_in_your_checkout_stop_the_merge_and_nothing_is_forced() {
    let w = world_with(Opts { repo: true, settings: json!({ "review.when": "never" }), ..Default::default() });
    let id = to_approval(&w, "Work");
    std::fs::write(w.root.join("mine.txt"), "unsaved\n").unwrap();
    finish_line::approve(&id, None, false).unwrap();
    let t = settle(&id);
    assert_eq!(t.s("stage"), "approval");
    assert_eq!(t.at("failure").s("title"), "Merge needs a clean checkout");
    assert_eq!(t.at("failure").at("agentCanFix"), &json!(false));
    let e = finish_line::send_failure(&id).unwrap_err().msg;
    assert!(e.contains("Commit, stash or move"), "{e}");
    assert!(w.root.join("mine.txt").exists());
}

#[test]
fn a_commit_that_reaches_the_branch_while_it_is_being_merged_is_neither_merged_unseen_nor_deleted_as_merged() {
    let w = world_with(Opts { repo: true, settings: json!({ "review.when": "never" }), ..Default::default() });
    let id = to_approval(&w, "Work");
    let t = task(&id);
    let tree = Path::new(t.s("worktree"));
    let reviewed = git(tree, &["rev-parse", "HEAD"]);
    // your checkout's pre-commit hook stands in for the agent: it commits to the task's branch in the middle of the merge
    let hook = w.root.join(".git/hooks/pre-commit");
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    std::fs::write(&hook, format!("#!/bin/sh\n[ -n \"$SW_LATE\" ] && exit 0\n(unset GIT_DIR GIT_INDEX_FILE GIT_WORK_TREE GIT_PREFIX; cd \"{}\" && echo late > late.txt && SW_LATE=1 git add -A && SW_LATE=1 git commit -qm late)\n", tree.display())).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    finish_line::approve(&id, None, false).unwrap();
    let t = settle(&id);
    assert_eq!(t.s("stage"), "done", "{}", t.at("failure"));
    assert!(regex::Regex::new(r"^Merged into main\. Kept: .*new work arrived during clean-up").unwrap().is_match(t.s("msg")), "{}", t.s("msg"));
    assert_eq!(read(&w.root.join("f.txt")), "v1\n");
    assert!(!w.root.join("late.txt").exists(), "what was reviewed landed, nothing else");
    assert!(tree.join("late.txt").exists());
    assert_eq!(git(&w.root, &["log", "-1", "--format=%s", t.s("branch")]), "late");
    assert_eq!(git(&w.root, &["rev-parse", &format!("{}~1", t.s("branch"))]), reviewed);
    assert!(!w.state().arr("workspaces").iter().any(|x| x.s("workspace_id") == t.at("link").s("workspace")));
}

#[test]
fn the_work_is_pushed_to_origin_and_your_checkout_is_left_as_it_was_even_with_unsaved_files() {
    let w = push_world(json!({}));
    let before = origin(&w, "main");
    // your checkout: unsaved changes, an untracked file, and a branch that is what it was
    std::fs::write(w.root.join("a.txt"), "yours\n").unwrap();
    std::fs::write(w.root.join("new.txt"), "untracked\n").unwrap();
    let mine = git(&w.root, &["rev-parse", "HEAD"]);
    let id = to_approval(&w, "Work");
    let tree = PathBuf::from(task(&id).t("worktree"));
    let t = approved(&id);
    assert_eq!((t.s("stage"), t.s("msg")), ("done", "Merged into main at origin (from origin/HEAD)."), "{}", t.at("failure"));
    assert_eq!((t.s("mergeInto"), t.s("mergeIntoFrom")), ("main", "origin/HEAD"), "where the target came from is on the task");
    let landed = origin(&w, "main");
    assert_eq!(t.s("mergeCommit"), landed, "the commit reported is the one origin shows");
    assert_ne!(landed, before);
    assert_eq!(git(&bare(&w), &["rev-parse", "main^"]), before, "one commit on top of main, not onto your own work");
    assert_eq!(has(&w, "main", "f.txt").as_deref(), Some("v1"));
    // your checkout is exactly as you left it: same branch, same commit, same files
    assert_eq!((git(&w.root, &["symbolic-ref", "--short", "HEAD"]), git(&w.root, &["rev-parse", "HEAD"])), ("main".into(), mine));
    assert_eq!((read(&w.root.join("a.txt")), read(&w.root.join("new.txt"))), ("yours\n".into(), "untracked\n".into()));
    assert!(!w.root.join("f.txt").exists(), "nothing was merged or checked out here");
    assert!(!tree.exists(), "and the task's own worktree is cleaned up");
}

#[test]
fn a_push_origin_refuses_fails_the_task_and_keeps_the_worktree() {
    let w = push_world(json!({}));
    let before = origin(&w, "main");
    let mine = git(&w.root, &["rev-parse", "HEAD"]);
    hook(&bare(&w), "pre-receive", "echo 'GL-HOOK-ERR: main is a protected branch' >&2\nexit 1\n");
    let id = to_approval(&w, "Refused");
    let tree = task(&id).t("worktree");
    let t = approved(&id);
    assert_eq!((t.s("stage"), t.at("failure").s("title")), ("approval", "Could not push to main at origin"), "{}", t.at("failure"));
    assert!(t.b("mergeFailed") && t.at("failure").s("detail").contains("main is a protected branch"), "{}", t.at("failure"));
    assert_eq!(t.at("failure").at("auth"), &json!(false));
    assert_eq!(origin(&w, "main"), before, "nothing landed");
    assert_eq!(git(&w.root, &["rev-parse", "HEAD"]), mine);
    assert!(Path::new(&tree).exists(), "the work is kept");
    // the way is cleared: the same task lands on a second yes
    std::fs::remove_file(bare(&w).join("hooks").join("pre-receive")).unwrap();
    let t = approved(&id);
    assert_eq!(t.s("stage"), "done", "{}", t.at("failure"));
    assert_eq!(t.s("mergeCommit"), origin(&w, "main"));
    assert_eq!(has(&w, "main", "f.txt").as_deref(), Some("v1"));
}

#[test]
fn a_push_the_remote_does_not_show_is_not_reported_as_merged_and_the_worktree_stays() {
    let w = push_world(json!({}));
    let before = origin(&w, "main");
    // origin takes the push and puts its branch back: git says it went, origin does not show that commit
    hook(&bare(&w), "post-receive", "while read old new ref; do git update-ref -m 'put back' \"$ref\" \"$old\"; done\n");
    let id = to_approval(&w, "Vanished");
    let tree = task(&id).t("worktree");
    let t = approved(&id);
    assert_eq!((t.s("stage"), t.at("failure").s("title")), ("approval", "The push to main is not what origin shows"), "{}", t.at("failure"));
    assert!(t.b("mergeFailed") && t.t("mergeCommit").is_empty(), "nothing is reported as merged");
    assert!(t.at("failure").s("detail").contains("origin/main is"), "{}", t.at("failure"));
    assert_eq!(origin(&w, "main"), before);
    assert!(Path::new(&tree).exists(), "the work is kept");
    std::fs::remove_file(bare(&w).join("hooks").join("post-receive")).unwrap();
    let t = approved(&id);
    assert_eq!(t.s("stage"), "done", "{}", t.at("failure"));
    assert_eq!(t.s("mergeCommit"), origin(&w, "main"));
}

#[test]
fn a_local_branch_that_is_behind_origin_is_not_the_base_the_work_lands_on() {
    let w = push_world(json!({}));
    // theirs is pushed while your local main stays behind: the board must land on origin's main, not on your copy
    let theirs = they_push(&w, "theirs.txt", "theirs\n");
    let stale = git(&w.root, &["rev-parse", "main"]);
    assert_ne!(theirs, stale);
    let id = to_approval(&w, "On the fresh main");
    let t = approved(&id);
    assert_eq!(t.s("stage"), "done", "{}", t.at("failure"));
    assert_eq!(t.s("mergeCommit"), origin(&w, "main"));
    assert_eq!(git(&bare(&w), &["rev-parse", "main^"]), theirs, "built on origin's main, not on your stale copy");
    assert_eq!(has(&w, "main", "theirs.txt").as_deref(), Some("theirs"));
    assert_eq!(has(&w, "main", "f.txt").as_deref(), Some("v1"));
    assert_eq!(git(&w.root, &["rev-parse", "main"]), stale, "your own copy of main is untouched, as stale as it was");
}

#[test]
fn a_checkout_on_another_branch_does_not_decide_where_the_work_lands() {
    let w = push_world(json!({}));
    let before = origin(&w, "main");
    // your checkout is on a feature branch of your own: that is never the branch the task lands on
    git(&w.root, &["checkout", "-q", "-b", "feature/x"]);
    let id = to_approval(&w, "On a feature branch");
    assert_eq!(sw::sync::lands_on(&task(&id)), "main", "the question before the merge says where it goes");
    let t = approved(&id);
    assert_eq!((t.s("stage"), t.s("msg")), ("done", "Merged into main at origin (from origin/HEAD)."), "{}", t.at("failure"));
    assert_eq!((t.s("mergeInto"), t.s("mergeIntoFrom")), ("main", "origin/HEAD"));
    let landed = origin(&w, "main");
    assert_eq!(t.s("mergeCommit"), landed); assert_ne!(landed, before);
    assert_eq!(has(&w, "main", "f.txt").as_deref(), Some("v1"), "the work landed on origin's main");
    assert_eq!(git(&w.root, &["symbolic-ref", "--short", "HEAD"]), "feature/x", "your checkout stays where it was");
    assert!(common::worktrees::git(&bare(&w), &["rev-parse", "--verify", "-q", "refs/heads/feature/x"]).is_err(), "nothing was pushed to your own branch");
}

#[test]
fn merge_into_names_the_branch_and_overrides_origins_default_branch() {
    let w = push_world(json!({ "merge.into": "develop" }));
    // the branch it names is at origin, and your checkout is somewhere else entirely
    git(&w.root, &["push", "-q", "origin", "main:develop"]);
    let (main, develop) = (origin(&w, "main"), git(&bare(&w), &["rev-parse", "develop"]));
    git(&w.root, &["checkout", "-q", "-b", "feature/x"]);
    let id = to_approval(&w, "Into develop");
    assert_eq!(sw::sync::lands_on(&task(&id)), "develop");
    let t = approved(&id);
    assert_eq!((t.s("stage"), t.s("msg")), ("done", "Merged into develop at origin (from merge.into)."), "{}", t.at("failure"));
    assert_eq!((t.s("mergeInto"), t.s("mergeIntoFrom")), ("develop", "merge.into"));
    let landed = git(&bare(&w), &["rev-parse", "develop"]);
    assert_ne!(landed, develop); assert_eq!(t.s("mergeCommit"), landed);
    assert_eq!(git(&bare(&w), &["rev-parse", "develop^"]), develop, "one commit on top of develop");
    assert_eq!(has(&w, "develop", "f.txt").as_deref(), Some("v1"));
    assert_eq!(origin(&w, "main"), main, "origin's default branch is untouched");
    assert_eq!(git(&w.root, &["symbolic-ref", "--short", "HEAD"]), "feature/x");
}

#[test]
fn nothing_naming_the_landing_branch_fails_and_pushes_nothing() {
    let w = push_world(json!({}));
    let id = to_approval(&w, "Nowhere");
    let before = origin(&w, "main");
    // origin/HEAD is forgotten, and origin itself stops saying which branch is its own: the board does not guess
    git(&w.root, &["remote", "set-head", "origin", "--delete"]);
    git(&bare(&w), &["symbolic-ref", "HEAD", "refs/heads/gone"]);
    assert_eq!(sw::sync::lands_on(&task(&id)), "no branch named");
    let t = approved(&id);
    // a failed merge waits for you again, saying why: nothing landed, and setting the branch makes the next yes work
    assert_eq!((t.s("stage"), t.at("failure").s("title")), ("approval", "Nothing says where this work lands"), "{}", t.at("failure"));
    assert!(t.b("mergeFailed") && !t.b("mergeIntent"), "{}", t.at("failure"));
    assert!(t.at("failure").s("detail").contains("origin/HEAD"), "{}", t.at("failure"));
    assert!(t.at("failure").s("next").contains("merge.into"), "{}", t.at("failure"));
    assert_eq!(t.at("failure").at("agentCanFix"), &json!(false));
    assert_eq!(origin(&w, "main"), before, "nothing was pushed");
    assert!(t.t("mergeCommit").is_empty());
    assert!(common::worktrees::git(&bare(&w), &["rev-parse", "--verify", "-q", &format!("refs/heads/{}", t.s("branch"))]).is_err(), "its own branch was not pushed either");
}
