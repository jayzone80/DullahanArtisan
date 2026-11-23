use std::collections::HashMap;
use std::f64::consts::PI;

/// Lightweight variable environment and arithmetic expression evaluator for DSL files.
/// Supports `$var = expr`, `+ - * / ^`, unary `-`, parentheses, and math functions:
/// `sin`, `cos`, `tan`, `atan2`, `sqrt`, `abs`, `min`, `max`, `floor`, `ceil`, `round`, `deg`, `rad`, `pi`.
#[derive(Debug, Clone, Default)]
pub struct ExprEnv {
    vars: HashMap<String, f64>,
}

impl ExprEnv {
    pub fn new() -> Self {
        Self {
            vars: HashMap::new(),
        }
    }

    pub fn set(&mut self, name: &str, value: f64) {
        let key = name.strip_prefix('$').unwrap_or(name);
        self.vars.insert(key.to_string(), value);
    }

    pub fn get(&self, name: &str) -> Option<f64> {
        let key = name.strip_prefix('$').unwrap_or(name);
        self.vars.get(key).copied()
    }

    pub fn eval(&self, input: &str) -> Result<f64, String> {
        let mut p = Parser {
            bytes: input.as_bytes(),
            pos: 0,
            env: self,
        };
        let v = p.parse_expr()?;
        p.skip_ws();
        if p.pos != p.bytes.len() {
            return Err(format!(
                "unexpected trailing token '{}' in expression '{}'",
                &input[p.pos..],
                input
            ));
        }
        if !v.is_finite() {
            return Err(format!("expression '{}' evaluated to non-finite value", input));
        }
        Ok(v)
    }

    pub fn eval_f32(&self, input: &str) -> Result<f32, String> {
        Ok(self.eval(input)? as f32)
    }

    /// Evaluate a comma-separated list of numeric expressions (e.g. `0,$h/2,-1.2`).
    pub fn eval_list_f32(&self, input: &str) -> Result<Vec<f32>, String> {
        let mut out = Vec::new();
        let mut start = 0usize;
        let mut depth = 0i32;
        for (i, b) in input.bytes().enumerate() {
            match b {
                b'(' => depth += 1,
                b')' => depth -= 1,
                b',' if depth == 0 => {
                    out.push(self.eval_f32(&input[start..i])?);
                    start = i + 1;
                }
                _ => {}
            }
        }
        if start <= input.len() {
            let tail = input[start..].trim();
            if !tail.is_empty() {
                out.push(self.eval_f32(tail)?);
            }
        }
        Ok(out)
    }
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
    env: &'a ExprEnv,
}

impl<'a> Parser<'a> {
    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.skip_ws();
        self.bytes.get(self.pos).copied()
    }

    fn parse_expr(&mut self) -> Result<f64, String> {
        let mut lhs = self.parse_term()?;
        loop {
            match self.peek() {
                Some(b'+') => {
                    self.pos += 1;
                    lhs += self.parse_term()?;
                }
                Some(b'-') => {
                    self.pos += 1;
                    lhs -= self.parse_term()?;
                }
                _ => break,
            }
        }
        Ok(lhs)
    }

    fn parse_term(&mut self) -> Result<f64, String> {
        let mut lhs = self.parse_power()?;
        loop {
            match self.peek() {
                Some(b'*') => {
                    self.pos += 1;
                    lhs *= self.parse_power()?;
                }
                Some(b'/') => {
                    self.pos += 1;
                    let rhs = self.parse_power()?;
                    if rhs.abs() < 1e-18 {
                        return Err("division by zero in expression".into());
                    }
                    lhs /= rhs;
                }
                _ => break,
            }
        }
        Ok(lhs)
    }

    fn parse_power(&mut self) -> Result<f64, String> {
        let base = self.parse_unary()?;
        if self.peek() == Some(b'^') {
            self.pos += 1;
            let exp = self.parse_power()?;
            Ok(base.powf(exp))
        } else {
            Ok(base)
        }
    }

    fn parse_unary(&mut self) -> Result<f64, String> {
        match self.peek() {
            Some(b'+') => {
                self.pos += 1;
                self.parse_unary()
            }
            Some(b'-') => {
                self.pos += 1;
                Ok(-self.parse_unary()?)
            }
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> Result<f64, String> {
        match self.peek() {
            Some(b'(') => {
                self.pos += 1;
                let v = self.parse_expr()?;
                if self.peek() != Some(b')') {
                    return Err("missing closing ')' in expression".into());
                }
                self.pos += 1;
                Ok(v)
            }
            Some(b'$') => {
                self.pos += 1;
                let name = self.read_ident();
                if name.is_empty() {
                    return Err("expected variable name after '$'".into());
                }
                self.env
                    .get(&name)
                    .ok_or_else(|| format!("undefined variable '${name}'"))
            }
            Some(b) if b.is_ascii_digit() || b == b'.' => self.read_number(),
            Some(b) if b.is_ascii_alphabetic() || b == b'_' => {
                let ident = self.read_ident();
                if self.peek() == Some(b'(') {
                    self.pos += 1;
                    let mut args = Vec::new();
                    if self.peek() != Some(b')') {
                        loop {
                            args.push(self.parse_expr()?);
                            if self.peek() == Some(b',') {
                                self.pos += 1;
                            } else {
                                break;
                            }
                        }
                    }
                    if self.peek() != Some(b')') {
                        return Err(format!("missing ')' after call to {ident}"));
                    }
                    self.pos += 1;
                    eval_fn(&ident, &args)
                } else {
                    match ident.as_str() {
                        "pi" | "PI" => Ok(PI),
                        _ => self
                            .env
                            .get(&ident)
                            .ok_or_else(|| format!("unknown identifier '{ident}'")),
                    }
                }
            }
            other => Err(format!(
                "unexpected token {:?} in expression",
                other.map(|c| c as char)
            )),
        }
    }

    fn read_ident(&mut self) -> String {
        let start = self.pos;
        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            if b.is_ascii_alphanumeric() || b == b'_' {
                self.pos += 1;
            } else {
                break;
            }
        }
        String::from_utf8_lossy(&self.bytes[start..self.pos]).into_owned()
    }

    fn read_number(&mut self) -> Result<f64, String> {
        let start = self.pos;
        while self.pos < self.bytes.len()
            && (self.bytes[self.pos].is_ascii_digit() || self.bytes[self.pos] == b'.')
        {
            self.pos += 1;
        }
        if self.pos < self.bytes.len()
            && (self.bytes[self.pos] == b'e' || self.bytes[self.pos] == b'E')
        {
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
        let s = std::str::from_utf8(&self.bytes[start..self.pos]).unwrap_or("");
        s.parse::<f64>()
            .map_err(|_| format!("invalid number '{s}'"))
    }
}

fn eval_fn(name: &str, args: &[f64]) -> Result<f64, String> {
    let check = |n: usize| -> Result<(), String> {
        if args.len() != n {
            Err(format!("function '{name}' expects {n} arg(s), got {}", args.len()))
        } else {
            Ok(())
        }
    };
    match name {
        "sin" => {
            check(1)?;
            Ok(args[0].sin())
        }
        "cos" => {
            check(1)?;
            Ok(args[0].cos())
        }
        "tan" => {
            check(1)?;
            Ok(args[0].tan())
        }
        "sind" => {
            check(1)?;
            Ok(args[0].to_radians().sin())
        }
        "cosd" => {
            check(1)?;
            Ok(args[0].to_radians().cos())
        }
        "atan2" => {
            check(2)?;
            Ok(args[0].atan2(args[1]))
        }
        "atan2d" => {
            check(2)?;
            Ok(args[0].atan2(args[1]).to_degrees())
        }
        "sqrt" => {
            check(1)?;
            Ok(args[0].sqrt())
        }
        "abs" => {
            check(1)?;
            Ok(args[0].abs())
        }
        "floor" => {
            check(1)?;
            Ok(args[0].floor())
        }
        "ceil" => {
            check(1)?;
            Ok(args[0].ceil())
        }
        "round" => {
            check(1)?;
            Ok(args[0].round())
        }
        "deg" => {
            check(1)?;
            Ok(args[0].to_degrees())
        }
        "rad" => {
            check(1)?;
            Ok(args[0].to_radians())
        }
        "min" => {
            check(2)?;
            Ok(args[0].min(args[1]))
        }
        "max" => {
            check(2)?;
            Ok(args[0].max(args[1]))
        }
        _ => Err(format!("unknown math function '{name}'")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expr_basics() {
        let mut env = ExprEnv::new();
        env.set("$h", 2.4);
        env.set("w", 1.5);
        assert!((env.eval("$h * 0.5 + $w").unwrap() - 2.7).abs() < 1e-9);
        assert!((env.eval("atan2d(1, 1)").unwrap() - 45.0).abs() < 1e-9);
        assert!((env.eval("2^3 + -1").unwrap() - 7.0).abs() < 1e-9);
        assert_eq!(env.eval_list_f32("0,$h/2,-$w").unwrap(), vec![0.0, 1.2, -1.5]);
    }
}
