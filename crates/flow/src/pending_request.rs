//! Public requests explicitly reported by a task's own worker or reviewer, and replies to that exact request.
use crate::clock::now;
use crate::err::Res;
use crate::json::*;
use crate::store::files::{with_lock, KINDS};
use crate::store::tasks::{find, get, update_wait, Change, Task};

fn author_link<'a>(t: &'a Task, role: &str) -> &'a Value {
    let link = t.at("link");
    if role == "reviewer" { link.at("reviewer") } else { link }
}

fn generation<'a>(t: &'a Task, role: &str) -> &'a Value {
    if role == "reviewer" { t.at("reviewAt") } else { t.at("startedAgentAt") }
}

/// The same public object in the CLI listing and the existing notification. Never project a transcript or a dialog.
pub fn pending(t: &Task) -> Option<&Value> {
    let r = t.at("request");
    let public = r.at("public");
    let role = public.s("role");
    (!t.b("archivedAt") && matches!((role, t.s("stage")), ("agent", "asks") | ("reviewer", "reviewneeds"))
        && author_link(t, role).at("pane") == r.at("pane") && t.at("link").at("socket") == r.at("socket")
        && generation(t, role) == r.at("generation"))
        .then_some(public)
}

pub(crate) fn active_attempt(t: &Task) -> bool {
    get(t.s("attemptOf")).is_some_and(|p| !p.b("archivedAt") && p.s("stage") == "attempts"
        && p.arr("attempts").iter().any(|entry| entry.at("id") == t.at("id")))
}
#[derive(Default)]
pub struct Words { pub options: Vec<String>, pub constraints: Vec<String>, pub pane: String, pub terminal: String }

pub(crate) fn record(t: &mut Task, role: &str, kind: &str, question: &str, words: &Words) -> Res<()> {
    let link = author_link(t, role);
    if words.pane.is_empty() || link.s("pane") != words.pane || !t.at("link").b("socket") || words.terminal.is_empty() {
        crate::fail!("Cannot verify the request author's terminal.");
    }
    if role == "reviewer" && (link.s("terminal") != words.terminal || !matches!(t.s("stage"), "reviewing" | "reviewneeds")) {
        crate::fail!("This reviewer is not reviewing the task now.");
    }
    if !kind.is_empty() && !KINDS.contains(&kind) { crate::fail!("Expected --kind question, permission or auth."); }
    if question.trim().is_empty() { crate::fail!("A public request needs its full question in --msg."); }
    let kind = if kind.is_empty() { "question" } else { kind };
    let public = json!({ "taskId": t.s("id"), "requestId": crate::util::random_hex(16), "role": role,
        "kind": kind, "question": question, "options": words.options, "constraints": words.constraints });
    // A single environment value must fit in the existing shell-command notification. Reject rather than truncate.
    if public.to_string().len() > 64 * 1024 { crate::fail!("Public request exceeds 64 KiB of JSON."); }
    t.put("request", json!({ "public": public, "terminal": words.terminal, "pane": link.at("pane"),
        "socket": t.at("link").at("socket"), "generation": generation(t, role) }));
    Ok(())
}

/// Close the reviewer's request without touching the worker's turn or the running review's guard.
pub(crate) fn reviewer_resumed(t: &mut Task, at: i64) {
    t.del("request");
    t.del("kind");
    t.put("stage", "reviewing");
    t.put("msg", "Under review");
    t.put("reviewReplyAt", at);
}

/// Never route a request reply through the generic `tell` path (which can restart or message the worker).
pub fn answer(id: &str, request_id: &str, text: &str, by: &str) -> Res<Task> {
    let id = find(id)?.t("id");
    if request_id.is_empty() || text.trim().is_empty() { crate::fail!("A request ID and the human's answer are required."); }
    // Serialize replies separately from the short-lived task lock: no herdr calls under the task lock.
    with_lock(&format!("request-reply:{id}"), || {
        let t = get(&id).ok_or("The task is gone.")?;
        if t.b("attemptOf") && !active_attempt(&t) { crate::fail!("That attempt is no longer active."); }
        let Some(public) = pending(&t).filter(|r| r.s("requestId") == request_id) else {
            crate::fail!("That request is no longer pending on this task.");
        };
        if public.s("kind") != "question" { crate::fail!("Open the author's pane for a permission or sign-in request."); }
        let request = t.at("request");
        let api = crate::flow::task::h(&t);
        let pane = request.s("pane");
        let live = api.agent_get(pane)?.filter(|a| a.at("terminal_id") == request.at("terminal"));
        let Some(live) = live else { crate::fail!("The request author's terminal is no longer available."); };
        if !matches!(live.s("agent_status"), "idle" | "done") { crate::fail!("Wait for the request author to finish their turn before answering."); }
        let reviewer = public.s("role") == "reviewer";
        let at = now();
        let mark = crate::submit::hand(&api, pane, text)?;
        crate::store::said_by::note_person(&id, by, text);
        let mut closed = false;
        let t = update_wait(&id, 10000, |t| {
            if !pending(t).is_some_and(|r| r.s("requestId") == request_id) { return Ok(Change::Keep); }
            if reviewer { reviewer_resumed(t, at); }
            else {
                crate::actions::talk::rearm(t, false, true, true)?;
                // A fast answer can finish before the prompt command returns. Its evidence is newer than sending began.
                t.put("armedAt", at);
            }
            closed = true;
            Ok(Change::Write)
        })?;
        if closed && !reviewer { crate::submit::note_sent(&id, mark, text, false); }
        Ok(t)
    })
}

pub(crate) fn report_rule(t: &Task, cmd: &str) -> String {
    format!("If you cannot continue without the human, run this from your own pane and then stop: {cmd} report {} --state blocked --kind <question|permission|auth> --msg \"<full public question>\". Include every public choice with --options '[\"first\",\"second\"]' and every public restriction with --constraints '[\"one choice\"]' (JSON arrays of strings). These words are published on the board: never include passwords, tokens, codes or other secrets. For sign-in or permissions, direct the human to your pane. Waiting for CI is not a question.", t.s("id"))
}

/// While a reviewer waits, its stop hook is not a verdict. A human may also resume it in its own terminal.
#[derive(Default)]
pub(crate) struct ReviewWait {
    id: String,
    since: Option<i64>,
    rest: Option<(Value, i64)>,
}
impl ReviewWait {
    pub(crate) fn waits(&mut self, t: &Task, a: Option<&Value>) -> Res<bool> {
        let Some(r) = pending(t).filter(|r| r.s("role") == "reviewer") else { return Ok(false); };
        if self.id != r.s("requestId") {
            self.id = r.t("requestId");
            self.since.get_or_insert_with(now);
            self.rest = None;
        }
        let Some(a) = a else { return Ok(false); };
        let status = a.s("agent_status");
        if let Some((comp, seq)) = &self.rest {
            if a.at("completion_seq") != comp || (status == "working" && a.i("state_change_seq") > *seq) {
                update_wait(t.s("id"), 10000, |t| {
                    if !pending(t).is_some_and(|r| r.s("requestId") == self.id) { return Ok(Change::Keep); }
                    reviewer_resumed(t, now());
                    Ok(Change::Write)
                })?;
            }
        } else if matches!(status, "idle" | "done") {
            self.rest = Some((a.at("completion_seq").clone(), a.i("state_change_seq")));
        }
        Ok(true)
    }
    pub(crate) fn resumed(&mut self, t: &Task) -> Option<i64> {
        if !t.b("reviewReplyAt") { return None; }
        self.since.take().map(|since| t.i("reviewReplyAt").saturating_sub(since).max(0))
    }
    pub(crate) fn completion(&mut self) -> Option<Value> { self.rest.take().map(|(comp, _)| comp) }
}
