//! Where a task's work lands (`merge.into`): the name of a branch at origin, or `none` for origin's own default branch.
//! Checked by git's own `check-ref-format` and refused with the nearest name it takes. Never `here`: the branch your
//! checkout happens to be on is never where work lands.
use super::branch::git_takes;
use super::from::{bare, mended};
use super::values::{nearest, NONE};
use crate::err::{Error, Res};
use crate::json::*;

fn refuse<T>(msg: String) -> Res<T> { Err(Error::refused(msg)) }

/// A branch's name at origin, or `none`, as kept; else refused with the nearest one that fits.
pub fn check(name: &str, v: &Value) -> Res<Value> {
    let Value::String(s) = v else { return refuse(format!("{name} is the name of a branch at origin, such as develop, or none for origin's default branch.")) };
    let w = s.trim();
    if w.to_lowercase() == NONE { return Ok(json!(NONE)); }
    if w.is_empty() { return refuse(format!("{name} is the name of a branch at origin, such as develop, or none for origin's default branch.")); }
    if w.eq_ignore_ascii_case("here") { return refuse(format!("{name} cannot be here: the branch your checkout happens to be on is never where work lands. Name the branch the work belongs to (main, develop), or none for origin's own default branch.")); }
    if w == "HEAD" || w.to_lowercase() == "origin/head" { return refuse(format!("{name}: \"{w}\" is no branch. Name the branch the work lands on (main, develop), or none for origin's own default branch.")); }
    if bare(w) != w && !bare(w).is_empty() && git_takes(bare(w)) { return refuse(format!("{name} is a branch's name at origin, without where it is: not \"{w}\". Did you mean {}?", bare(w))); }
    if !w.starts_with('-') && !w.contains("@{") && git_takes(w) { return Ok(json!(w)); }
    let near = mended(w);
    let hint = if !near.is_empty() && git_takes(&near) { format!(" Did you mean {near}?") } else { nearest(w, &["main", "develop"]).map(|n| format!(" Did you mean {n}?")).unwrap_or_default() };
    refuse(format!("{name}: \"{w}\" is not a branch name git takes. It is the name of a branch at origin, such as develop, or none for origin's own default branch.{hint}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn values() {
        let ok = |v: &str| check("merge.into", &json!(v)).unwrap();
        let bad = |v: &str| check("merge.into", &json!(v)).unwrap_err().msg;
        assert_eq!((ok(" None "), ok("develop"), ok("release/2.0"), ok("main")), (json!("none"), json!("develop"), json!("release/2.0"), json!("main")));
        assert!(bad("origin/develop").contains("Did you mean develop?"), "{}", bad("origin/develop"));
        assert!(bad("refs/heads/develop").contains("Did you mean develop?"));
        assert!(bad("here").contains("never where work lands"));
        assert!(bad("HEAD").contains("is no branch"));
        assert!(bad("my branch").contains("Did you mean my-branch?"), "{}", bad("my branch"));
        assert!(bad("-x").contains("Did you mean x?"), "{}", bad("-x"));
    }
}
