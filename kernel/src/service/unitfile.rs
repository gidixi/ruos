//! UnitDoc: modello intermedio comune ai parser YAML/JSON, e builder
//! UnitDoc → Unit|Timer.
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use super::{Unit, Timer, UnitKind, RestartPolicy, ActivateTarget, UnitStatus};
use super::schedule::schedule_parse;

#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    Str(String),
    Bool(bool),
    List(Vec<String>),
}

#[derive(Clone, Debug, Default)]
pub struct UnitDoc(pub Vec<(String, Val)>);

impl UnitDoc {
    pub fn get(&self, key: &str) -> Option<&Val> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
    pub fn str_of(&self, key: &str) -> Option<&str> {
        match self.get(key) { Some(Val::Str(s)) => Some(s.as_str()), _ => None }
    }
    pub fn bool_of(&self, key: &str) -> Option<bool> {
        match self.get(key) { Some(Val::Bool(b)) => Some(*b), _ => None }
    }
    pub fn list_of(&self, key: &str) -> Option<&[String]> {
        match self.get(key) { Some(Val::List(l)) => Some(l.as_slice()), _ => None }
    }
}

pub enum Parsed { U(Unit), T(Timer) }

/// UnitDoc → Unit | Timer. `kind: timer` discrimina. Chiavi sconosciute:
/// warn e prosegue. Errori (campi mancanti/valori invalidi) → Err(msg).
pub fn build(doc: &UnitDoc, file: Option<&str>) -> Result<Parsed, String> {
    const KNOWN: &[&str] = &["name", "kind", "type", "exec", "restart", "target",
                             "enabled", "after", "requires", "unit", "schedule"];
    for (k, _) in &doc.0 {
        if !KNOWN.contains(&k.as_str()) {
            crate::bwarn!("svc", "unitfile: unknown key '{}' (ignored)", k);
        }
    }
    let name = doc.str_of("name").ok_or_else(|| "missing 'name'".to_string())?.to_string();

    if doc.str_of("kind") == Some("timer") {
        let unit = doc.str_of("unit").ok_or_else(|| "timer: missing 'unit'".to_string())?.to_string();
        let schedule = schedule_parse(
            doc.str_of("schedule").ok_or_else(|| "timer: missing 'schedule'".to_string())?
        )?;
        return Ok(Parsed::T(Timer {
            name, unit, schedule,
            enabled: doc.bool_of("enabled").unwrap_or(false),
            next_fire: 0,            // armato da load_from_disk/insert
            last_fire: None,
            file: file.map(|s| s.to_string()),
        }));
    }

    let path = doc.str_of("exec").ok_or_else(|| "missing 'exec'".to_string())?.to_string();
    let kind = match doc.str_of("type").unwrap_or("oneshot") {
        "oneshot" => UnitKind::Oneshot,
        "daemon"  => UnitKind::Daemon,
        other => return Err(alloc::format!("bad type '{}'", other)),
    };
    let restart = match doc.str_of("restart").unwrap_or("no") {
        "no"         => RestartPolicy::No,
        "on-failure" => RestartPolicy::OnFailure,
        "always"     => RestartPolicy::Always,
        other => return Err(alloc::format!("bad restart '{}'", other)),
    };
    let target = match doc.str_of("target").unwrap_or("manual") {
        "boot"      => ActivateTarget::Boot,
        "post-boot" => ActivateTarget::PostBoot,
        "manual"    => ActivateTarget::Manual,
        other => return Err(alloc::format!("bad target '{}'", other)),
    };
    let to_vec = |k: &str| doc.list_of(k).map(|l| l.to_vec()).unwrap_or_default();
    Ok(Parsed::U(Unit {
        name, path, kind, restart,
        after: to_vec("after"), requires: to_vec("requires"),
        target,
        enabled: doc.bool_of("enabled").unwrap_or(false),
        status: UnitStatus::Idle, pid: None, runs: 0, restarts: 0,
        stop_requested: false,
        file: file.map(|s| s.to_string()),
    }))
}

fn kind_str(k: UnitKind) -> &'static str {
    match k { UnitKind::Oneshot => "oneshot", UnitKind::Daemon => "daemon" }
}
fn restart_str(r: RestartPolicy) -> &'static str {
    match r { RestartPolicy::No => "no", RestartPolicy::OnFailure => "on-failure", RestartPolicy::Always => "always" }
}
fn target_str(t: ActivateTarget) -> &'static str {
    match t { ActivateTarget::Boot => "boot", ActivateTarget::PostBoot => "post-boot", ActivateTarget::Manual => "manual" }
}

/// Serializza una Unit nel formato YAML-subset (per la persistenza di
/// enable/disable: riscrittura del file sorgente).
pub fn to_yaml(u: &Unit) -> String {
    let mut s = alloc::format!(
        "name: {}\ntype: {}\nexec: {}\nrestart: {}\ntarget: {}\nenabled: {}\n",
        u.name, kind_str(u.kind), u.path, restart_str(u.restart),
        target_str(u.target), u.enabled);
    if !u.after.is_empty()    { s += &alloc::format!("after: [{}]\n",    u.after.join(", ")); }
    if !u.requires.is_empty() { s += &alloc::format!("requires: [{}]\n", u.requires.join(", ")); }
    s
}

/// Serializza una Unit nel formato JSON-subset.
pub fn to_json(u: &Unit) -> String {
    let list = |l: &[String]| -> String {
        let items: Vec<String> = l.iter().map(|x| alloc::format!("\"{}\"", x)).collect();
        alloc::format!("[{}]", items.join(","))
    };
    alloc::format!(
        "{{ \"name\":\"{}\", \"type\":\"{}\", \"exec\":\"{}\", \"restart\":\"{}\", \"target\":\"{}\", \"enabled\":{}, \"after\":{}, \"requires\":{} }}\n",
        u.name, kind_str(u.kind), u.path, restart_str(u.restart),
        target_str(u.target), u.enabled, list(&u.after), list(&u.requires))
}
