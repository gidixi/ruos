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
