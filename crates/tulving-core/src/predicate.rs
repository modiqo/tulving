//! jq predicates over run results, evaluated with the embedded jaq
//! engine. `prev` in a predicate names the previous run's result, so
//! `.dau < prev.dau * 0.95` fires on a five-percent drop.

use anyhow::{anyhow, bail, Result};
use jaq_core::load::{Arena, File, Loader};
use jaq_core::{Compiler, Ctx, RcIter};
use jaq_json::Val;

/// Check that a predicate parses and compiles, without running it.
/// Crystallization calls this so a typo fails now, not at 7:30 tomorrow.
pub fn validate(predicate: &str) -> Result<()> {
    let code = bind_prev(predicate);
    let program = File {
        code: code.as_str(),
        path: (),
    };
    let loader = Loader::new(jaq_std::defs().chain(jaq_json::defs()));
    let arena = Arena::default();
    let modules = loader.load(&arena, program).map_err(|errs| {
        anyhow!(
            "cannot parse predicate '{predicate}': {} error(s)",
            errs.len()
        )
    })?;
    Compiler::default()
        .with_funs(jaq_std::funs().chain(jaq_json::funs()))
        .with_global_vars(["$prev"])
        .compile(modules)
        .map_err(|errs| {
            anyhow!(
                "cannot compile predicate '{predicate}': {} error(s)",
                errs.len()
            )
        })?;
    Ok(())
}

/// Evaluate a jq predicate against `input`, binding `$prev` to the
/// previous result. Returns true when the first output is neither
/// `null` nor `false`; an empty output stream is false.
pub fn eval_bool(
    predicate: &str,
    input: &serde_json::Value,
    prev: &serde_json::Value,
) -> Result<bool> {
    let code = bind_prev(predicate);
    let program = File {
        code: code.as_str(),
        path: (),
    };
    let loader = Loader::new(jaq_std::defs().chain(jaq_json::defs()));
    let arena = Arena::default();
    let modules = loader.load(&arena, program).map_err(|errs| {
        anyhow!(
            "cannot parse predicate '{predicate}': {} error(s)",
            errs.len()
        )
    })?;
    let filter = Compiler::default()
        .with_funs(jaq_std::funs().chain(jaq_json::funs()))
        .with_global_vars(["$prev"])
        .compile(modules)
        .map_err(|errs| {
            anyhow!(
                "cannot compile predicate '{predicate}': {} error(s)",
                errs.len()
            )
        })?;
    let inputs = RcIter::new(core::iter::empty());
    let ctx = Ctx::new([Val::from(prev.clone())], &inputs);
    let mut outputs = filter.run((ctx, Val::from(input.clone())));
    match outputs.next() {
        Some(Ok(value)) => Ok(!matches!(value, Val::Null | Val::Bool(false))),
        Some(Err(err)) => bail!("predicate '{predicate}' failed: {err}"),
        None => Ok(false),
    }
}

/// Rewrite bare `prev` to jq's `$prev` so predicates read naturally.
/// A `prev` already prefixed with `$` or `.`, or inside a longer
/// identifier, is left alone.
fn bind_prev(predicate: &str) -> String {
    let chars: Vec<char> = predicate.chars().collect();
    let mut out = String::with_capacity(predicate.len() + 4);
    let mut i = 0;
    while i < chars.len() {
        let is_prev =
            chars.get(i..i + 4).map(|w| w.iter().collect::<String>()) == Some("prev".to_string());
        let before_ok = i == 0
            || chars
                .get(i.wrapping_sub(1))
                .is_none_or(|c| !c.is_alphanumeric() && *c != '_' && *c != '$' && *c != '.');
        let after_ok = chars
            .get(i + 4)
            .is_none_or(|c| !c.is_alphanumeric() && *c != '_');
        if is_prev && before_ok && after_ok {
            out.push_str("$prev");
            i += 4;
        } else if let Some(c) = chars.get(i) {
            out.push(*c);
            i += 1;
        } else {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn truthiness_matches_jq() {
        let v = json!({"dau": 100});
        let none = json!(null);
        assert!(eval_bool(".dau == 100", &v, &none).unwrap());
        assert!(!eval_bool(".dau == 99", &v, &none).unwrap());
        assert!(!eval_bool(".missing", &v, &none).unwrap());
    }

    #[test]
    fn prev_binds_to_previous_result() {
        let now = json!({"dau": 90});
        let before = json!({"dau": 100});
        assert!(eval_bool(".dau < prev.dau * 0.95", &now, &before).unwrap());
        assert!(!eval_bool(".dau < prev.dau * 0.5", &now, &before).unwrap());
    }

    #[test]
    fn prev_rewrite_leaves_identifiers_alone() {
        assert_eq!(bind_prev("prev.dau"), "$prev.dau");
        assert_eq!(bind_prev("$prev.dau"), "$prev.dau");
        assert_eq!(bind_prev(".preview"), ".preview");
        assert_eq!(bind_prev(".a.prev"), ".a.prev");
    }

    #[test]
    fn bad_predicate_is_an_error() {
        assert!(eval_bool(".dau ==", &json!({}), &json!(null)).is_err());
    }
}
