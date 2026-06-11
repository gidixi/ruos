//! Parser YAML-subset line-based: `key: value`, liste inline `[a, b]`,
//! commenti `#`, righe vuote. Niente nesting, niente multiline. Vedi spec
//! init-units §2.
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use super::unitfile::{UnitDoc, Val};

pub fn parse(src: &str) -> Result<UnitDoc, String> {
    let mut doc = UnitDoc::default();
    for (i, raw) in src.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        let (key, val) = line.split_once(':')
            .ok_or_else(|| alloc::format!("line {}: missing ':'", i + 1))?;
        let key = key.trim().to_string();
        let val = strip_comment(val).trim();
        doc.0.push((key, parse_val(val)));
    }
    Ok(doc)
}

/// Taglia un commento ` # ...` (subset: '#' non è ammesso dentro i valori).
fn strip_comment(v: &str) -> &str {
    match v.find('#') { Some(i) => &v[..i], None => v }
}

fn parse_val(v: &str) -> Val {
    if v == "true"  { return Val::Bool(true); }
    if v == "false" { return Val::Bool(false); }
    if let Some(inner) = v.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        let items = inner.split(',')
            .map(|s| unquote(s.trim()).to_string())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        return Val::List(items);
    }
    Val::Str(unquote(v).to_string())
}

fn unquote(v: &str) -> &str {
    let v = v.trim();
    v.strip_prefix('"').and_then(|s| s.strip_suffix('"'))
        .or_else(|| v.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
        .unwrap_or(v)
}
