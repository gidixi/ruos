//! Parser JSON-subset: UN oggetto piatto { "k": v }, v ∈ stringa | bool |
//! numero (tenuto come stringa) | array di stringhe. Char-scanner, zero dep.
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use super::unitfile::{UnitDoc, Val};

struct P<'a> { b: &'a [u8], i: usize }

pub fn parse(src: &str) -> Result<UnitDoc, String> {
    let mut p = P { b: src.as_bytes(), i: 0 };
    p.ws();
    p.expect(b'{')?;
    let mut doc = UnitDoc::default();
    p.ws();
    if p.peek() == Some(b'}') { p.i += 1; return Ok(doc); }
    loop {
        p.ws();
        let key = p.string()?;
        p.ws();
        p.expect(b':')?;
        p.ws();
        let val = p.value()?;
        doc.0.push((key, val));
        p.ws();
        match p.next() {
            Some(b',') => continue,
            Some(b'}') => return Ok(doc),
            _ => return Err("expected ',' or '}'".to_string()),
        }
    }
}

impl<'a> P<'a> {
    fn peek(&self) -> Option<u8> { self.b.get(self.i).copied() }
    fn next(&mut self) -> Option<u8> { let c = self.peek(); if c.is_some() { self.i += 1; } c }
    fn ws(&mut self) { while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) { self.i += 1; } }
    fn expect(&mut self, c: u8) -> Result<(), String> {
        if self.next() == Some(c) { Ok(()) } else { Err(alloc::format!("expected '{}'", c as char)) }
    }
    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let start = self.i;
        while let Some(c) = self.peek() {
            if c == b'"' {
                let s = core::str::from_utf8(&self.b[start..self.i])
                    .map_err(|_| "bad utf8".to_string())?.to_string();
                self.i += 1;
                return Ok(s);
            }
            if c == b'\\' { return Err("escape not supported".to_string()); }
            self.i += 1;
        }
        Err("unterminated string".to_string())
    }
    fn value(&mut self) -> Result<Val, String> {
        match self.peek() {
            Some(b'"') => Ok(Val::Str(self.string()?)),
            Some(b'[') => {
                self.i += 1;
                let mut items = Vec::new();
                self.ws();
                if self.peek() == Some(b']') { self.i += 1; return Ok(Val::List(items)); }
                loop {
                    self.ws();
                    items.push(self.string()?);
                    self.ws();
                    match self.next() {
                        Some(b',') => continue,
                        Some(b']') => return Ok(Val::List(items)),
                        _ => return Err("expected ',' or ']'".to_string()),
                    }
                }
            }
            Some(b't') if self.b[self.i..].starts_with(b"true")  => { self.i += 4; Ok(Val::Bool(true)) }
            Some(b'f') if self.b[self.i..].starts_with(b"false") => { self.i += 5; Ok(Val::Bool(false)) }
            Some(c) if c == b'-' || c.is_ascii_digit() => {
                let start = self.i;
                while matches!(self.peek(), Some(c) if c == b'-' || c == b'.' || c.is_ascii_digit()) { self.i += 1; }
                Ok(Val::Str(core::str::from_utf8(&self.b[start..self.i]).unwrap_or("0").to_string()))
            }
            _ => Err("unexpected value".to_string()),
        }
    }
}
