//! UnitDoc: modello intermedio comune ai parser YAML/JSON, e builder
//! UnitDoc → Unit|Timer (builder al Task 5 del piano init-units).
use alloc::string::String;
use alloc::vec::Vec;

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
