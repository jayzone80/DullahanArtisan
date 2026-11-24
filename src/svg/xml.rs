/// Minimal zero-dependency XML tree parser with line-numbered diagnostics for SVG files.

#[derive(Debug, Clone)]
pub struct XmlNode {
    pub tag: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<XmlNode>,
    pub line: usize,
}

impl XmlNode {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

struct Cursor<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
    line: usize,
}

impl<'a> Cursor<'a> {
    fn new(src: &'a str) -> Self {
        let trimmed = src.strip_prefix('\u{feff}').unwrap_or(src);
        Self {
            src: trimmed,
            bytes: trimmed.as_bytes(),
            pos: 0,
            line: 1,
        }
    }

    fn eof(&self) -> bool {
        self.pos >= self.bytes.len()
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn starts_with(&self, s: &str) -> bool {
        self.src[self.pos..].starts_with(s)
    }

    fn advance(&mut self, n: usize) {
        for &b in &self.bytes[self.pos..self.pos + n] {
            if b == b'\n' {
                self.line += 1;
            }
        }
        self.pos += n;
    }

    fn skip_ws(&mut self) {
        while let Some(b) = self.peek() {
            if b.is_ascii_whitespace() {
                self.advance(1);
            } else {
                break;
            }
        }
    }

    fn skip_misc(&mut self) -> Result<(), String> {
        loop {
            self.skip_ws();
            if self.starts_with("<!--") {
                let start_line = self.line;
                self.advance(4);
                if let Some(rel) = self.src[self.pos..].find("-->") {
                    self.advance(rel + 3);
                } else {
                    return Err(format!("XML line {start_line}: unclosed comment"));
                }
            } else if self.starts_with("<?") {
                let start_line = self.line;
                self.advance(2);
                if let Some(rel) = self.src[self.pos..].find("?>") {
                    self.advance(rel + 2);
                } else {
                    return Err(format!("XML line {start_line}: unclosed processing instruction"));
                }
            } else if self.starts_with("<!DOCTYPE") || self.starts_with("<!doctype") {
                let start_line = self.line;
                self.advance(9);
                let mut depth = 1usize;
                while !self.eof() && depth > 0 {
                    match self.peek().unwrap() {
                        b'<' => {
                            depth += 1;
                            self.advance(1);
                        }
                        b'>' => {
                            depth -= 1;
                            self.advance(1);
                        }
                        _ => self.advance(1),
                    }
                }
                if depth != 0 {
                    return Err(format!("XML line {start_line}: unclosed DOCTYPE"));
                }
            } else {
                break;
            }
        }
        Ok(())
    }

    fn read_name(&mut self) -> Result<String, String> {
        let start = self.pos;
        while let Some(b) = self.peek() {
            if b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b':' | b'.') {
                self.advance(1);
            } else {
                break;
            }
        }
        if self.pos == start {
            return Err(format!("XML line {}: expected identifier", self.line));
        }
        Ok(self.src[start..self.pos].to_string())
    }

    fn read_attr_value(&mut self) -> Result<String, String> {
        let start_line = self.line;
        let quote = self
            .peek()
            .filter(|&b| b == b'"' || b == b'\'')
            .ok_or_else(|| format!("XML line {start_line}: expected quoted attribute value"))?;
        self.advance(1);
        let start = self.pos;
        while let Some(b) = self.peek() {
            if b == quote {
                let raw = &self.src[start..self.pos];
                self.advance(1);
                return Ok(unescape_xml(raw));
            }
            self.advance(1);
        }
        Err(format!("XML line {start_line}: unclosed attribute string"))
    }

    fn parse_element(&mut self) -> Result<XmlNode, String> {
        self.skip_misc()?;
        let start_line = self.line;
        if self.peek() != Some(b'<') {
            return Err(format!("XML line {start_line}: expected '<'"));
        }
        self.advance(1);
        if matches!(self.peek(), Some(b'/') | Some(b'!') | Some(b'?')) {
            return Err(format!("XML line {start_line}: unexpected tag syntax"));
        }

        let tag = self.read_name()?;
        let mut attrs = Vec::new();

        loop {
            self.skip_ws();
            if self.eof() {
                return Err(format!("XML line {start_line}: unclosed opening tag <{tag}>"));
            }
            if self.starts_with("/>") {
                self.advance(2);
                return Ok(XmlNode {
                    tag,
                    attrs,
                    children: Vec::new(),
                    line: start_line,
                });
            }
            if self.peek() == Some(b'>') {
                self.advance(1);
                break;
            }
            let k = self.read_name()?;
            self.skip_ws();
            if self.peek() != Some(b'=') {
                return Err(format!(
                    "XML line {}: expected '=' after attribute '{k}' in <{tag}>",
                    self.line
                ));
            }
            self.advance(1);
            self.skip_ws();
            let v = self.read_attr_value()?;
            attrs.push((k, v));
        }

        let mut children = Vec::new();
        loop {
            // Skip text content and CDATA/comments between child elements
            while !self.eof() {
                if self.starts_with("<![CDATA[") {
                    let cd_line = self.line;
                    self.advance(9);
                    if let Some(rel) = self.src[self.pos..].find("]]>") {
                        self.advance(rel + 3);
                    } else {
                        return Err(format!("XML line {cd_line}: unclosed CDATA section"));
                    }
                } else if self.starts_with("<!--") {
                    let cm_line = self.line;
                    self.advance(4);
                    if let Some(rel) = self.src[self.pos..].find("-->") {
                        self.advance(rel + 3);
                    } else {
                        return Err(format!("XML line {cm_line}: unclosed comment"));
                    }
                } else if self.starts_with("<?") {
                    let pi_line = self.line;
                    self.advance(2);
                    if let Some(rel) = self.src[self.pos..].find("?>") {
                        self.advance(rel + 2);
                    } else {
                        return Err(format!("XML line {pi_line}: unclosed processing instruction"));
                    }
                } else if self.peek() == Some(b'<') {
                    break;
                } else {
                    self.advance(1);
                }
            }

            if self.eof() {
                return Err(format!(
                    "XML line {start_line}: unclosed element <{tag}> (reached EOF)"
                ));
            }

            if self.starts_with("</") {
                let close_line = self.line;
                self.advance(2);
                self.skip_ws();
                let close_tag = self.read_name()?;
                self.skip_ws();
                if self.peek() != Some(b'>') {
                    return Err(format!(
                        "XML line {close_line}: expected '>' in closing tag </{close_tag}>"
                    ));
                }
                self.advance(1);
                if close_tag != tag {
                    return Err(format!(
                        "XML line {close_line}: mismatched closing tag </{close_tag}> for <{tag}> opened on line {start_line}"
                    ));
                }
                break;
            }

            children.push(self.parse_element()?);
        }

        Ok(XmlNode {
            tag,
            attrs,
            children,
            line: start_line,
        })
    }
}

fn unescape_xml(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(idx) = rest.find('&') {
        out.push_str(&rest[..idx]);
        rest = &rest[idx + 1..];
        if let Some(semi) = rest.find(';') {
            let ent = &rest[..semi];
            match ent {
                "amp" => out.push('&'),
                "lt" => out.push('<'),
                "gt" => out.push('>'),
                "quot" => out.push('"'),
                "apos" => out.push('\''),
                _ if ent.starts_with("#x") || ent.starts_with("#X") => {
                    if let Some(ch) = u32::from_str_radix(&ent[2..], 16)
                        .ok()
                        .and_then(char::from_u32)
                    {
                        out.push(ch);
                    }
                }
                _ if ent.starts_with('#') => {
                    if let Some(ch) = ent[1..].parse::<u32>().ok().and_then(char::from_u32) {
                        out.push(ch);
                    }
                }
                _ => {
                    out.push('&');
                    out.push_str(ent);
                    out.push(';');
                }
            }
            rest = &rest[semi + 1..];
        } else {
            out.push('&');
        }
    }
    out.push_str(rest);
    out
}

pub fn parse_xml(src: &str) -> Result<XmlNode, String> {
    let mut cur = Cursor::new(src);
    cur.skip_misc()?;
    if cur.eof() {
        return Err("empty XML/SVG document".into());
    }
    let root = cur.parse_element()?;
    cur.skip_misc()?;
    if !cur.eof() {
        return Err(format!("XML line {}: trailing content after root element", cur.line));
    }
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_basic_and_errors() {
        let doc = parse_xml(
            "\u{feff}<?xml version=\"1.0\"?><!-- hi --><svg viewBox=\"0 0 16 16\"><rect x=\"1\" y=\"2\" width=\"10\" height=\"10\"/></svg>",
        )
        .unwrap();
        assert_eq!(doc.tag, "svg");
        assert_eq!(doc.attr("viewBox"), Some("0 0 16 16"));
        assert_eq!(doc.children.len(), 1);
        assert_eq!(doc.children[0].tag, "rect");

        assert!(parse_xml("<svg><g></svg>").is_err());
        assert!(parse_xml("<svg><!-- unclosed").is_err());
    }
}
