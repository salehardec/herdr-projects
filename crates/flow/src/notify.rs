//! Telling you that a task needs you, is ready to approve, or failed: the `board.notify` setting. `bell` rings the
//! terminal bell where a board is open; a command is run once per event, with the task's id, its title and what
//! happened in its environment; `none` does neither.
use crate::clock::now;
use crate::json::*;
use crate::pending_request::pending;
use crate::store::attempts::attempts_of;
use crate::store::settings::lookup::setting_text;
use crate::store::tasks::{update, Change};

/// What happened, from where the task stands; "" for a task nobody needs to hear about.
pub fn event_of(t: &Obj) -> &'static str {
    if t.b("archivedAt") { return ""; }
    let st = t.s("stage");
    if ["asks", "reviewneeds", "plan", "pick"].contains(&st) || (st == "stopped" && t.b("limitStop")) || (st == "after" && t.b("afterBlocked")) { return "needs you"; }
    if st == "attempts" { return if attempts_of(t).iter().any(|c| ["asks", "reviewneeds"].contains(&c.s("stage"))) { "needs you" } else { "" }; }
    if st == "failed" || st == "nosummary" { return "failed"; }
    if st == "approval" { return if t.b("failure") || t.b("prFailure") || t.b("syncBlocked") { "needs you" } else { "ready to approve" }; }
    ""
}
fn request_id(t: &Obj) -> &str { pending(t).map(|r| r.s("requestId")).unwrap_or("") }
fn notice_event(t: &Obj) -> &'static str {
    // Keep the parent's normal events; individual attempts notify only for a public, addressable request.
    if t.b("attemptOf") && request_id(t).is_empty() { "" } else { event_of(t) }
}
fn noted(t: &Obj, event: &str) -> bool {
    let notice = t.at("notice");
    notice.s("event") == event && (event.is_empty() || (notice.s("stage") == t.s("stage") && notice.s("requestId") == request_id(t)))
}
/// The housekeeping pass: each event is noted on its task once (`notice`), and the command is run for it.
pub fn notify_pass(tasks: &[Obj]) {
    let how = setting_text("board.notify", None);
    for seen in tasks {
        if noted(seen, notice_event(seen)) { continue; }
        if seen.b("attemptOf") && pending(seen).is_some() && !crate::pending_request::active_attempt(seen) { continue; }
        let mut fresh = false;
        let Ok(t) = update(seen.s("id"), |t| {
            let ev = notice_event(t);
            if noted(t, ev) { return Ok(Change::Keep); }
            t.put("notice", if ev.is_empty() { Value::Null } else { json!({ "event": ev, "stage": t.s("stage"), "at": now(), "requestId": request_id(t) }) });
            fresh = !ev.is_empty();
            Ok(Change::Write)
        }) else { continue };
        if !fresh || how == "none" || how == "bell" { continue; }
        let env = [
            ("HERDR_PROJECTS_ID", t.t("id")), ("HERDR_PROJECTS_TITLE", t.t("title")),
            ("HERDR_PROJECTS_EVENT", t.at("notice").t("event")),
            ("HERDR_PROJECTS_REQUEST", pending(&t).map(Value::to_string).unwrap_or_default()),
        ];
        crate::runner::detached_sh(&how, if env[3].1.is_empty() { &env[..3] } else { &env });
    }
}
