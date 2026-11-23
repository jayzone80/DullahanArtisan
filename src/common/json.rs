use std::collections::BTreeMap;

/// Minimal zero-dependency JSON AST, parser, and deterministic serializer (for glTF 2.0 export & validation).
#[derive(Debug, Clone)]
pub enum JsonVal {
    Null,
    Bool(bool),
    Num(f64),
    Int(i64),
    Str(String),
    Arr(Vec<JsonVal>),
    Obj(Vec<(String, JsonVal)>),
}

impl JsonVal {
    pub fn as_obj(&self) -> Option<&[(String, JsonVal)]> {
        match self {
            JsonVal::Obj(v) => Some(v),
            _ => None,
        }
    }
    pub fn get(&self, key: &str) -> Option<&JsonVal> {
        self.as_obj()?.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
    pub fn as_arr(&self) -> Option<&[JsonVal]> {
        match self {
            JsonVal::Arr(v) => Some(v),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            JsonVal::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            JsonVal::Num(n) => Some(*n),
            JsonVal::Int(i) => Some(*i as f64),
            _ => None,
        }
    }
    pub fn as_usize(&self) -> Option<usize> {
        match self {
            JsonVal::Int(i) if *i >= 0 => Some(*i as usize),
            JsonVal::Num(n) if *n >= 0.0 && (*n - n.trunc()).abs() < 1e-9 => Some(*n as usize),
            _ => None,
        }
    }

    pub fn to_string_compact(&self) -> String {
        let mut out = String::new();
        self.write_to(&mut out);
        out
    }

    fn write_to(&self, out: &mut String) {
        match self {
            JsonVal::Null => out.push_str("null"),
            JsonVal::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            JsonVal::Int(i) => {
                use std::fmt::Write;
                let _ = write!(out, "{i}");
            }
            JsonVal::Num(n) => {
                use std::fmt::Write;
                if !n.is_finite() {
                    out.push_str("0");
                } else if (*n - n.round()).abs() < 1e-9 && n.abs() < 1e12 {
                    let _ = write!(out, "{:.1}", n);
                } else {
                    let s = format!("{:.6}", n);
                    let trimmed = s.trim_end_matches('0').trim_end_matches('.');
                    out.push_str(if trimmed == "-0" { "0" } else { trimmed });
                }
            }
            JsonVal::Str(s) => write_escaped_str(out, s),
            JsonVal::Arr(items) => {
                out.push('[');
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    v.write_to(out);
                }
                out.push(']');
            }
            JsonVal::Obj(entries) => {
                out.push('{');
                for (i, (k, v)) in entries.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_escaped_str(out, k);
                    out.push(':');
                    v.write_to(out);
                }
                out.push('}');
            }
        }
    }
}

fn write_escaped_str(out: &mut String, s: &str) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                use std::fmt::Write;
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

pub fn parse_json(input: &str) -> Result<JsonVal, String> {
    let mut p = JsonParser {
        bytes: input.as_bytes(),
        pos: 0,
    };
    let v = p.parse_val()?;
    p.skip_ws();
    if p.pos != p.bytes.len() {
        return Err("trailing characters after JSON root".into());
    }
    Ok(v)
}

struct JsonParser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> JsonParser<'a> {
    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }
    fn peek(&mut self) -> Option<u8> {
        self.skip_ws();
        self.bytes.get(self.pos).copied()
    }
    fn parse_val(&mut self) -> Result<JsonVal, String> {
        match self.peek() {
            Some(b'{') => self.parse_obj(),
            Some(b'[') => self.parse_arr(),
            Some(b'"') => Ok(JsonVal::Str(self.parse_str()?)),
            Some(b't') => {
                self.expect_lit(b"true")?;
                Ok(JsonVal::Bool(true))
            }
            Some(b'f') => {
                self.expect_lit(b"false")?;
                Ok(JsonVal::Bool(false))
            }
            Some(b'n') => {
                self.expect_lit(b"null")?;
                Ok(JsonVal::Null)
            }
            Some(b) if b == b'-' || b.is_ascii_digit() => self.parse_num(),
            other => Err(format!("unexpected JSON token {:?}", other.map(|c| c as char))),
        }
    }
    fn expect_lit(&mut self, lit: &[u8]) -> Result<(), String> {
        if self.bytes[self.pos..].starts_with(lit) {
            self.pos += lit.len();
            Ok(())
        } else {
            Err("invalid JSON literal".into())
        }
    }
    fn parse_str(&mut self) -> Result<String, String> {
        self.pos += 1; // skip '"'
        let mut out = String::new();
        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            self.pos += 1;
            if b == b'"' {
                return Ok(out);
            }
            if b == b'\\' {
                if self.pos >= self.bytes.len() {
                    return Err("unterminated escape".into());
                }
                let esc = self.bytes[self.pos];
                self.pos += 1;
                match esc {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'u' => {
                        if self.pos + 4 > self.bytes.len() {
                            return Err("truncated \\u escape".into());
                        }
                        let hex = std::str::from_utf8(&self.bytes[self.pos..self.pos + 4])
                            .map_err(|_| "bad utf8")?;
                        let cp = u32::from_str_radix(hex, 16).map_err(|_| "bad \\u hex")?;
                        if let Some(ch) = char::from_u32(cp) {
                            out.push(ch);
                        }
                        self.pos += 4;
                    }
                    _ => return Err("invalid JSON escape".into()),
                }
            } else {
                // Handle UTF-8 multi-byte slices directly
                let start = self.pos - 1;
                while self.pos < self.bytes.len()
                    && self.bytes[self.pos] != b'"'
                    && self.bytes[self.pos] != b'\\'
                {
                    self.pos += 1;
                }
                let chunk = std::str::from_utf8(&self.bytes[start..self.pos])
                    .map_err(|_| "invalid utf8 in JSON string")?;
                out.push_str(chunk);
            }
        }
        Err("unterminated JSON string".into())
    }
    fn parse_num(&mut self) -> Result<JsonVal, String> {
        let start = self.pos;
        let mut is_float = false;
        if self.bytes[self.pos] == b'-' {
            self.pos += 1;
        }
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
            self.pos += 1;
        }
        if self.pos < self.bytes.len() && self.bytes[self.pos] == b'.' {
            is_float = true;
            self.pos += 1;
            while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
        }
        if self.pos < self.bytes.len()
            && (self.bytes[self.pos] == b'e' || self.bytes[self.pos] == b'E')
        {
            is_float = true;
            self.pos += 1;
            if self.pos < self.bytes.len()
                && (self.bytes[self.pos] == b'+' || self.bytes[self.pos] == b'-')
            {
                self.pos += 1;
            }
            while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
        }
        let s = std::str::from_utf8(&self.bytes[start..self.pos]).map_err(|_| "bad num")?;
        if !is_float {
            if let Ok(i) = s.parse::<i64>() {
                return Ok(JsonVal::Int(i));
            }
        }
        let n = s.parse::<f64>().map_err(|_| format!("invalid JSON number '{s}'"))?;
        Ok(JsonVal::Num(n))
    }
    fn parse_arr(&mut self) -> Result<JsonVal, String> {
        self.pos += 1;
        let mut out = Vec::new();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(JsonVal::Arr(out));
        }
        loop {
            out.push(self.parse_val()?);
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    break;
                }
                _ => return Err("expected ',' or ']' in JSON array".into()),
            }
        }
        Ok(JsonVal::Arr(out))
    }
    fn parse_obj(&mut self) -> Result<JsonVal, String> {
        self.pos += 1;
        let mut out = Vec::new();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(JsonVal::Obj(out));
        }
        loop {
            if self.peek() != Some(b'"') {
                return Err("expected string key in JSON object".into());
            }
            let k = self.parse_str()?;
            if self.peek() != Some(b':') {
                return Err("expected ':' after JSON key".into());
            }
            self.pos += 1;
            let v = self.parse_val()?;
            out.push((k, v));
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    break;
                }
                _ => return Err("expected ',' or '}' in JSON object".into()),
            }
        }
        Ok(JsonVal::Obj(out))
    }
}

pub fn obj(entries: Vec<(&str, JsonVal)>) -> JsonVal {
    JsonVal::Obj(entries.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

#[allow(dead_code)]
pub fn obj_map(map: BTreeMap<String, JsonVal>) -> JsonVal {
    JsonVal::Obj(map.into_iter().collect())
}
