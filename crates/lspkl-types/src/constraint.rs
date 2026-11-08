//! Pure, conservative static analysis of type constraints.
//!
//! No query or I/O is performed here. Unsupported expressions retain their source
//! and evaluate to `Undecidable`, never to a speculative failure or proof.
use std::{cmp::Ordering, fmt::Display};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Truth {
    True,
    False,
    Undecidable,
}
impl Truth {
    fn from_bool(value: bool) -> Self {
        if value { Self::True } else { Self::False }
    }
    fn not(self) -> Self {
        match self {
            Self::True => Self::False,
            Self::False => Self::True,
            Self::Undecidable => self,
        }
    }
    fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::False, _) | (_, Self::False) => Self::False,
            (Self::True, Self::True) => Self::True,
            _ => Self::Undecidable,
        }
    }
    fn or(self, other: Self) -> Self {
        match (self, other) {
            (Self::True, _) | (_, Self::True) => Self::True,
            (Self::False, Self::False) => Self::False,
            _ => Self::Undecidable,
        }
    }
}

/// A Pkl quantity keeps its original magnitude and unit conversion factor.
/// Conversion order affects IEEE-754 rounding and must not be normalized away.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quantity {
    pub value: f64,
    pub factor: f64,
}
impl std::ops::Neg for Quantity {
    type Output = Self;
    fn neg(self) -> Self {
        Self {
            value: -self.value,
            ..self
        }
    }
}
impl Quantity {
    fn compare(self, other: Self) -> Ordering {
        let (a, b) = if self.factor <= other.factor {
            (self.value * self.factor / other.factor, other.value)
        } else {
            (self.value, other.value * other.factor / self.factor)
        };
        // Java Double.compare distinguishes signed zero (numeric == does not).
        a.total_cmp(&b)
    }
    fn equals(self, other: Self) -> bool {
        self.value * self.factor / 1.0 == other.value * other.factor / 1.0
    }
}

/// Values available without executing a Pkl program.
#[derive(Clone, Debug, PartialEq)]
pub enum StaticValue {
    Int(i64),
    Float(f64),
    String(String),
    Boolean(bool),
    Duration(Quantity),
    DataSize(Quantity),
    CollectionLength(usize),
}

/// Source rendering is deliberately independent of the parsed representation.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConstraintExpr {
    source: String,
    expression: Expr,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Expr {
    Unknown,
    This,
    Length,
    Literal(String),
    Call(String, Vec<Expr>),
    Compare(String, Box<Expr>, Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
}
impl ConstraintExpr {
    /// Preserve source verbatim and parse the supported static subset.
    pub fn opaque(text: impl Into<String>) -> Self {
        let source = text.into();
        let expression = parse(&source).unwrap_or(Expr::Unknown);
        Self { source, expression }
    }
    /// Keep a context-dependent expression uninterpreted even if its syntax is
    /// recognized (for example when a builtin constructor may be shadowed).
    pub(crate) fn uninterpreted(text: impl Into<String>) -> Self {
        Self {
            source: text.into(),
            expression: Expr::Unknown,
        }
    }
    /// Whether evaluation depends on an unqualified receiver member whose
    /// lexical binding must be checked by the type-resolution adapter.
    pub(crate) fn has_implicit_members(&self) -> bool {
        let Some(tokens) = tokenize(&self.source) else {
            return true;
        };
        tokens.iter().enumerate().any(|(index, token)| {
            matches!(
                token.0.as_str(),
                "length"
                    | "isPositive"
                    | "isNonZero"
                    | "isBetween"
                    | "isEven"
                    | "isOdd"
                    | "isEmpty"
                    | "startsWith"
                    | "endsWith"
                    | "contains"
                    | "matches"
            ) && !(index >= 2
                && tokens.get(index - 1).is_some_and(|t| t.0 == ".")
                && tokens.get(index - 2).is_some_and(|t| t.0 == "this"))
        })
    }
    pub fn evaluate(&self, value: &StaticValue) -> Truth {
        self.expression.evaluate(value)
    }
}
impl Display for ConstraintExpr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.source)
    }
}

/// Parse a literal only: arbitrary expressions and string interpolation are not
/// evaluated. Integer overflow and non-finite floating point values are unknown.
pub fn parse_literal(text: &str) -> Option<StaticValue> {
    let text = text.trim();
    if text == "true" {
        return Some(StaticValue::Boolean(true));
    }
    if text == "false" {
        return Some(StaticValue::Boolean(false));
    }
    if text.starts_with('"') || text.starts_with('#') {
        return string_literal(text).map(StaticValue::String);
    }
    if let Some((number, unit)) = text.rsplit_once('.') {
        let factor = match unit {
            "ns" => Some((1.0, true)),
            "us" => Some((1e3, true)),
            "ms" => Some((1e6, true)),
            "s" => Some((1e9, true)),
            "min" => Some((60e9, true)),
            "h" => Some((3600e9, true)),
            "d" => Some((86400e9, true)),
            "b" => Some((1.0, false)),
            "kb" => Some((1e3, false)),
            "mb" => Some((1e6, false)),
            "gb" => Some((1e9, false)),
            "tb" => Some((1e12, false)),
            "pb" => Some((1e15, false)),
            "kib" => Some((1024.0, false)),
            "mib" => Some((1048576.0, false)),
            "gib" => Some((1073741824.0, false)),
            "tib" => Some((1099511627776.0, false)),
            "pib" => Some((1125899906842624.0, false)),
            _ => None,
        };
        if let Some((factor, duration)) = factor {
            let number = number.trim();
            // Unary minus acts on the quantity, preserving negative integer zero.
            let unsigned = number.strip_prefix('-').map(str::trim).unwrap_or(number);
            let value = match parse_number(unsigned)? {
                StaticValue::Int(n) => n as f64,
                StaticValue::Float(n) => n,
                _ => return None,
            };
            let value = if number.starts_with('-') {
                -value
            } else {
                value
            };
            let quantity = Quantity { value, factor };
            return Some(if duration {
                StaticValue::Duration(quantity)
            } else {
                StaticValue::DataSize(quantity)
            });
        }
    }
    parse_number(text)
}

fn parse_number(text: &str) -> Option<StaticValue> {
    let normalized = text.strip_prefix('-').map(str::trim).unwrap_or(text);
    if normalized.is_empty() || normalized.starts_with(['-', '+']) {
        return None;
    }
    // Only Pkl's numeric literal grammar; do not accept Rust's NaN/inf forms.
    if !normalized
        .chars()
        .all(|c| c.is_ascii_hexdigit() || matches!(c, '_' | '.' | '+' | '-' | 'x' | 'o'))
    {
        return None;
    }
    let normalized = normalized.replace('_', "");
    let negative = text.starts_with('-');
    let (digits, radix) = if let Some(n) = normalized.strip_prefix("0x") {
        (n, 16)
    } else if let Some(n) = normalized.strip_prefix("0o") {
        (n, 8)
    } else if let Some(n) = normalized.strip_prefix("0b") {
        (n, 2)
    } else {
        (normalized.as_str(), 10)
    };
    if radix != 10 || !normalized.contains(['.', 'e', 'E']) {
        let n = i128::from_str_radix(digits, radix).ok()?;
        return i64::try_from(if negative { -n } else { n })
            .ok()
            .map(StaticValue::Int);
    }
    let n = normalized.parse::<f64>().ok()?;
    let n = if negative { -n } else { n };
    n.is_finite().then_some(StaticValue::Float(n))
}

fn string_literal(text: &str) -> Option<String> {
    let hashes = text.bytes().take_while(|b| *b == b'#').count();
    let rest = &text[hashes..];
    if !rest.starts_with('"') || rest.starts_with("\"\"\"") {
        return None;
    }
    let suffix = format!("\"{}", "#".repeat(hashes));
    let body = rest.strip_prefix('"')?.strip_suffix(&suffix)?;
    let marker = format!("\\{}", "#".repeat(hashes));
    let mut result = String::new();
    let mut remaining = body;
    while !remaining.is_empty() {
        if remaining.starts_with(&suffix) {
            return None;
        }
        if let Some(after) = remaining.strip_prefix(&marker) {
            let c = after.chars().next()?;
            let decoded = match c {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                '"' => '"',
                '\\' => '\\',
                'u' => {
                    let unicode = after.strip_prefix("u{")?;
                    let end = unicode.find('}')?;
                    let scalar = u32::from_str_radix(&unicode[..end], 16).ok()?;
                    result.push(char::from_u32(scalar)?);
                    remaining = &unicode[end + 1..];
                    continue;
                }
                _ => return None, // Includes interpolation.
            };
            result.push(decoded);
            remaining = &after[c.len_utf8()..];
        } else {
            let c = remaining.chars().next()?;
            if c == '\n' || c == '\r' || (hashes == 0 && c == '"') {
                return None;
            }
            result.push(c);
            remaining = &remaining[c.len_utf8()..];
        }
    }
    Some(result)
}

#[derive(Clone, Debug)]
struct Token(String);
fn tokenize(text: &str) -> Option<Vec<Token>> {
    let mut tokens = Vec::new();
    let mut i = 0;
    let bytes = text.as_bytes();
    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        if bytes[i] == b'"' || bytes[i] == b'#' {
            let hashes = bytes[i..].iter().take_while(|b| **b == b'#').count();
            i += hashes;
            if bytes.get(i) != Some(&b'"') {
                return None;
            }
            i += 1;
            let mut closed = false;
            while i < bytes.len() {
                if bytes[i] == b'\\'
                    && bytes
                        .get(i + 1..i + 1 + hashes)
                        .is_some_and(|s| s.iter().all(|b| *b == b'#'))
                {
                    i += 1 + hashes;
                    if i < bytes.len() {
                        i += 1;
                    }
                } else if bytes[i] == b'"'
                    && bytes
                        .get(i + 1..i + 1 + hashes)
                        .is_some_and(|s| s.iter().all(|b| *b == b'#'))
                {
                    i += 1 + hashes;
                    closed = true;
                    break;
                } else {
                    i += 1;
                }
            }
            if !closed {
                return None;
            }
        } else if bytes[i].is_ascii_digit() {
            i += 1;
            while i < bytes.len() {
                let c = bytes[i];
                if c.is_ascii_alphanumeric()
                    || c == b'_'
                    || (c == b'.' && bytes.get(i + 1).is_some_and(u8::is_ascii_digit))
                    || ((c == b'+' || c == b'-') && matches!(bytes[i - 1], b'e' | b'E'))
                {
                    i += 1;
                } else {
                    break;
                }
            }
        } else if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
        } else {
            if !bytes[i].is_ascii() {
                return None;
            }
            i += 1;
            if i < bytes.len()
                && bytes[i].is_ascii()
                && matches!(&text[start..=i], "&&" | "||" | "==" | "!=" | ">=" | "<=")
            {
                i += 1;
            }
            if !text.is_char_boundary(i) {
                return None;
            }
        }
        tokens.push(Token(text[start..i].to_owned()));
        if tokens.len() > 1024 {
            return None;
        }
    }
    Some(tokens)
}
struct Parser {
    tokens: Vec<Token>,
    index: usize,
}
impl Parser {
    fn eat(&mut self, value: &str) -> bool {
        if self.tokens.get(self.index).is_some_and(|t| t.0 == value) {
            self.index += 1;
            true
        } else {
            false
        }
    }
    fn next(&mut self) -> Option<String> {
        let token = self.tokens.get(self.index)?.0.clone();
        self.index += 1;
        Some(token)
    }
    fn expression(&mut self, depth: usize) -> Option<Expr> {
        if depth > 64 {
            return None;
        }
        let mut left = self.conjunction(depth + 1)?;
        while self.eat("||") {
            left = Expr::Or(Box::new(left), Box::new(self.conjunction(depth + 1)?));
        }
        Some(left)
    }
    fn conjunction(&mut self, depth: usize) -> Option<Expr> {
        let mut left = self.comparison(depth)?;
        while self.eat("&&") {
            left = Expr::And(Box::new(left), Box::new(self.comparison(depth)?));
        }
        Some(left)
    }
    fn comparison(&mut self, depth: usize) -> Option<Expr> {
        let left = self.atom(depth)?;
        if let Some(op) = self.tokens.get(self.index).map(|t| t.0.clone())
            && matches!(op.as_str(), "<" | ">" | "<=" | ">=" | "==" | "!=")
        {
            self.index += 1;
            return Some(Expr::Compare(
                op,
                Box::new(left),
                Box::new(self.atom(depth)?),
            ));
        }
        Some(left)
    }
    fn atom(&mut self, depth: usize) -> Option<Expr> {
        if depth > 64 {
            return None;
        }
        if self.eat("!") {
            return Some(Expr::Not(Box::new(self.atom(depth + 1)?)));
        }
        if self.eat("(") {
            let result = self.expression(depth + 1)?;
            return self.eat(")").then_some(result);
        }
        let negative = self.eat("-");
        let mut name = self.next()?;
        if negative {
            name.insert(0, '-');
        }
        if parse_literal(&name).is_some() {
            if self.eat(".") {
                name.push('.');
                name.push_str(&self.next()?);
            }
            return parse_literal(&name).map(|_| Expr::Literal(name));
        }
        if name == "this" && !self.eat(".") {
            return Some(Expr::This);
        }
        if name == "this" {
            name = self.next()?;
        }
        if name == "length" {
            return Some(Expr::Length);
        }
        let mut args = Vec::new();
        if self.eat("(") && !self.eat(")") {
            loop {
                args.push(self.expression(depth + 1)?);
                if self.eat(")") {
                    break;
                }
                if !self.eat(",") {
                    return None;
                }
            }
        }
        Some(Expr::Call(name, args))
    }
}
fn parse(text: &str) -> Option<Expr> {
    // Bound both retained parser work and recursive expression depth.
    if text.len() > 16384 {
        return None;
    }
    let tokens = tokenize(text)?;
    let mut parser = Parser { tokens, index: 0 };
    let result = parser.expression(0)?;
    (parser.index == parser.tokens.len()).then_some(result)
}

impl Expr {
    fn value(&self, this: &StaticValue) -> Option<StaticValue> {
        match self {
            Self::This => Some(this.clone()),
            Self::Length => match this {
                StaticValue::String(s) => {
                    i64::try_from(s.chars().count()).ok().map(StaticValue::Int)
                }
                StaticValue::CollectionLength(n) => i64::try_from(*n).ok().map(StaticValue::Int),
                _ => None,
            },
            Self::Literal(s) => parse_literal(s),
            _ => None,
        }
    }
    fn evaluate(&self, this: &StaticValue) -> Truth {
        match self {
            Self::And(a, b) => a.evaluate(this).and(b.evaluate(this)),
            Self::Or(a, b) => a.evaluate(this).or(b.evaluate(this)),
            Self::Not(a) => a.evaluate(this).not(),
            Self::Compare(op, a, b) => match (a.value(this), b.value(this)) {
                (Some(a), Some(b)) if op == "==" || op == "!=" => equal(&a, &b)
                    .map(|eq| Truth::from_bool(if op == "==" { eq } else { !eq }))
                    .unwrap_or(Truth::Undecidable),
                (Some(a), Some(b)) => compare(&a, &b)
                    .map(|order| {
                        Truth::from_bool(match op.as_str() {
                            "<" => order.is_lt(),
                            ">" => order.is_gt(),
                            "<=" => !order.is_gt(),
                            ">=" => !order.is_lt(),
                            "==" => order.is_eq(),
                            "!=" => !order.is_eq(),
                            _ => return Truth::Undecidable,
                        })
                    })
                    .unwrap_or(Truth::Undecidable),
                _ => Truth::Undecidable,
            },
            Self::Call(name, args) => evaluate_call(name, args, this),
            _ => match self.value(this) {
                Some(StaticValue::Boolean(b)) => Truth::from_bool(b),
                _ => Truth::Undecidable,
            },
        }
    }
}

fn compare(a: &StaticValue, b: &StaticValue) -> Option<Ordering> {
    match (a, b) {
        (StaticValue::Int(a), StaticValue::Int(b)) => Some(a.cmp(b)),
        // Do not round large integers into floating point and invent equality.
        (StaticValue::Int(a), StaticValue::Float(b)) if a.unsigned_abs() <= (1u64 << 53) => {
            (*a as f64).partial_cmp(b)
        }
        (StaticValue::Float(a), StaticValue::Int(b)) if b.unsigned_abs() <= (1u64 << 53) => {
            a.partial_cmp(&(*b as f64))
        }
        (StaticValue::Float(a), StaticValue::Float(b)) => a.partial_cmp(b),
        (StaticValue::Duration(a), StaticValue::Duration(b))
        | (StaticValue::DataSize(a), StaticValue::DataSize(b)) => Some(a.compare(*b)),
        (StaticValue::String(a), StaticValue::String(b)) => {
            Some(a.encode_utf16().cmp(b.encode_utf16()))
        }
        (StaticValue::Boolean(a), StaticValue::Boolean(b)) => Some(a.cmp(b)),
        _ => None,
    }
}
fn equal(a: &StaticValue, b: &StaticValue) -> Option<bool> {
    match (a, b) {
        (StaticValue::Duration(a), StaticValue::Duration(b))
        | (StaticValue::DataSize(a), StaticValue::DataSize(b)) => Some(a.equals(*b)),
        _ => compare(a, b).map(Ordering::is_eq),
    }
}
fn zero(this: &StaticValue) -> Option<StaticValue> {
    match this {
        StaticValue::Int(_) => Some(StaticValue::Int(0)),
        StaticValue::Float(_) => Some(StaticValue::Float(0.0)),

        _ => None,
    }
}
fn evaluate_call(name: &str, args: &[Expr], this: &StaticValue) -> Truth {
    let result = match (name, args) {
        ("isPositive", [])
            if matches!(this, StaticValue::Duration(_) | StaticValue::DataSize(_)) =>
        {
            match this {
                StaticValue::Duration(q) | StaticValue::DataSize(q) => Some(q.value >= 0.0),
                _ => None,
            }
        }
        ("isNonZero", [])
            if matches!(this, StaticValue::Duration(_) | StaticValue::DataSize(_)) =>
        {
            match this {
                StaticValue::Duration(q) | StaticValue::DataSize(q) => Some(q.value != 0.0),
                _ => None,
            }
        }
        ("isPositive", []) => zero(this)
            .and_then(|z| compare(this, &z))
            .map(|o| !o.is_lt()),
        ("isNonZero", []) => zero(this)
            .and_then(|z| compare(this, &z))
            .map(|o| !o.is_eq()),
        ("isEven", []) => {
            if let StaticValue::Int(n) = this {
                Some(n % 2 == 0)
            } else {
                None
            }
        }
        ("isOdd", []) => {
            if let StaticValue::Int(n) = this {
                Some(n % 2 != 0)
            } else {
                None
            }
        }
        ("isBetween", [min, max]) => min.value(this).and_then(|min| {
            max.value(this).and_then(|max| {
                Some(!compare(this, &min)?.is_lt() && !compare(this, &max)?.is_gt())
            })
        }),
        ("isEmpty", []) => match this {
            StaticValue::String(s) => Some(s.is_empty()),
            StaticValue::CollectionLength(n) => Some(*n == 0),
            _ => None,
        },
        ("startsWith" | "endsWith" | "contains", [arg]) => match (this, arg.value(this)) {
            (StaticValue::String(s), Some(StaticValue::String(arg))) => Some(match name {
                "startsWith" => s.starts_with(&arg),
                "endsWith" => s.ends_with(&arg),
                _ => s.contains(&arg),
            }),
            _ => None,
        },
        ("matches", [Expr::Call(regex, args)]) if regex == "Regex" => match (this, args.as_slice())
        {
            (StaticValue::String(s), [Expr::Literal(pattern)]) => match parse_literal(pattern) {
                Some(StaticValue::String(pattern)) => regex_matches(&pattern, s),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    };
    result.map(Truth::from_bool).unwrap_or(Truth::Undecidable)
}

fn regex_matches(pattern: &str, value: &str) -> Option<bool> {
    // Pkl uses Java regexes. Only evaluate the shared ASCII subset: Java's
    // character classes, flags, Unicode and line terminators differ from Rust's.
    if pattern.len() > 4096
        || !pattern.is_ascii()
        || !value.is_ascii()
        || value.bytes().any(|b| b < 32 || b == 127)
        || pattern.contains("(?")
        || pattern.contains("&&")
        || pattern.contains("--")
        || pattern.contains("~~")
        || pattern.contains("||")
        || ["*+", "++", "?+", "}+"]
            .iter()
            .any(|token| pattern.contains(token))
    {
        return None;
    }
    let mut escaped = false;
    let mut in_class = false;
    for c in pattern.chars() {
        if escaped {
            if c.is_ascii_alphanumeric() && c != 'd' && c != 'D' {
                return None;
            }
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == '[' {
            if in_class {
                return None;
            }
            in_class = true;
        } else if c == ']' {
            in_class = false;
        }
    }
    let regex = regex::RegexBuilder::new(&format!(r"\A(?:{pattern})\z"))
        .size_limit(1 << 20)
        .build()
        .ok()?;
    Some(regex.is_match(value))
}

#[derive(Clone, Default)]
struct Interval {
    lower: Option<(StaticValue, bool)>,
    upper: Option<(StaticValue, bool)>,
}
fn bound_implies(
    source: &Option<(StaticValue, bool)>,
    target: &Option<(StaticValue, bool)>,
    lower: bool,
) -> bool {
    let Some((target_value, target_inclusive)) = target else {
        return true;
    };
    let Some((source_value, source_inclusive)) = source else {
        return false;
    };
    match compare(source_value, target_value) {
        Some(Ordering::Equal) => *target_inclusive || !*source_inclusive,
        Some(Ordering::Greater) => lower,
        Some(Ordering::Less) => !lower,
        None => false,
    }
}
impl Interval {
    fn implies(&self, other: &Self) -> bool {
        bound_implies(&self.lower, &other.lower, true)
            && bound_implies(&self.upper, &other.upper, false)
    }
    fn intersect(&mut self, other: &Self) {
        if other.lower.is_some()
            && (self.lower.is_none() || bound_implies(&other.lower, &self.lower, true))
        {
            self.lower.clone_from(&other.lower);
        }
        if other.upper.is_some()
            && (self.upper.is_none() || bound_implies(&other.upper, &self.upper, false))
        {
            self.upper.clone_from(&other.upper);
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Subject {
    This,
    Length,
}
fn interval_literal(text: &str) -> Option<StaticValue> {
    // Quantity comparisons convert units and round before ordering; their
    // equality is a different base-unit relation. Do not assume interval laws.
    parse_literal(text)
        .filter(|v| !matches!(v, StaticValue::Duration(_) | StaticValue::DataSize(_)))
}
fn interval(expr: &Expr) -> Option<(Subject, Interval)> {
    match expr {
        Expr::Compare(op, a, b) => {
            let subject = |e: &Expr| match e {
                Expr::This => Some(Subject::This),
                Expr::Length => Some(Subject::Length),
                _ => None,
            };
            let literal = |e: &Expr| match e {
                Expr::Literal(s) => interval_literal(s),
                _ => None,
            };
            let (subject, value, reverse) = if let (Some(s), Some(v)) = (subject(a), literal(b)) {
                (s, v, false)
            } else if let (Some(s), Some(v)) = (subject(b), literal(a)) {
                (s, v, true)
            } else {
                return None;
            };
            let op = if reverse {
                match op.as_str() {
                    "<" => ">",
                    ">" => "<",
                    "<=" => ">=",
                    ">=" => "<=",
                    op => op,
                }
            } else {
                op
            };
            let mut range = Interval::default();
            match op {
                ">" | ">=" => range.lower = Some((value, op == ">=")),
                "<" | "<=" => range.upper = Some((value, op == "<=")),
                "==" => {
                    range.lower = Some((value.clone(), true));
                    range.upper = Some((value, true));
                }
                _ => return None,
            }
            Some((subject, range))
        }
        Expr::Call(name, args) if name == "isPositive" && args.is_empty() => Some((
            Subject::This,
            Interval {
                lower: Some((StaticValue::Int(0), true)),
                upper: None,
            },
        )),
        Expr::Call(name, args) if name == "isBetween" => match args.as_slice() {
            [Expr::Literal(a), Expr::Literal(b)] => Some((
                Subject::This,
                Interval {
                    lower: Some((interval_literal(a)?, true)),
                    upper: Some((interval_literal(b)?, true)),
                },
            )),
            _ => None,
        },
        Expr::Call(name, args) if name == "isEmpty" && args.is_empty() => Some((
            Subject::Length,
            Interval {
                lower: Some((StaticValue::Int(0), true)),
                upper: Some((StaticValue::Int(0), true)),
            },
        )),
        _ => None,
    }
}
fn conjuncts<'a>(expr: &'a Expr, out: &mut Vec<&'a Expr>) {
    if let Expr::And(a, b) = expr {
        conjuncts(a, out);
        conjuncts(b, out);
    } else {
        out.push(expr);
    }
}
fn proves(source: &[&Expr], target: &Expr) -> bool {
    if source.contains(&target) {
        return true;
    }
    match target {
        Expr::Literal(text) if text == "true" => true,
        Expr::And(a, b) => proves(source, a) && proves(source, b),
        Expr::Or(a, b) => proves(source, a) || proves(source, b),
        _ => {
            if let Some((subject, target_interval)) = interval(target) {
                let mut range = Interval::default();
                for source in source {
                    if let Some((s, source_interval)) = interval(source)
                        && s == subject
                    {
                        range.intersect(&source_interval);
                    }
                }
                return range.implies(&target_interval);
            }
            false
        }
    }
}
/// Prove conjunction implication. Failure to prove is not a counterexample.
/// Opaque constraints can only imply themselves; unrelated unknowns never prove
/// a stronger refinement. The proof combines interval bounds in conjunctions.
pub fn constraints_imply(source: &[ConstraintExpr], target: &[ConstraintExpr]) -> bool {
    let mut terms = Vec::new();
    for expr in source {
        conjuncts(&expr.expression, &mut terms);
    }
    target.iter().all(|target| {
        // Unknown AST nodes carry no identity; compare their source, not Unknown.
        if target.expression == Expr::Unknown {
            source.iter().any(|s| s.source == target.source)
        } else {
            proves(&terms, &target.expression)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn evaluate(expr: &str, literal: &str) -> Truth {
        ConstraintExpr::opaque(expr).evaluate(&parse_literal(literal).unwrap())
    }
    fn implies(source: &[&str], target: &[&str]) -> bool {
        constraints_imply(
            &source
                .iter()
                .map(|s| ConstraintExpr::opaque(*s))
                .collect::<Vec<_>>(),
            &target
                .iter()
                .map(|s| ConstraintExpr::opaque(*s))
                .collect::<Vec<_>>(),
        )
    }
    #[test]
    fn constraint_literals() {
        assert_eq!(parse_literal("- 42"), Some(StaticValue::Int(-42)));
        assert_eq!(
            parse_literal("-9223372036854775808"),
            Some(StaticValue::Int(i64::MIN))
        );
        assert_eq!(parse_literal(&format!("1{}", ".s".repeat(10000))), None);
        assert_eq!(parse_literal("0xff"), Some(StaticValue::Int(255)));
        assert_eq!(parse_literal("0b101"), Some(StaticValue::Int(5)));
        assert_eq!(parse_literal("1.5e2"), Some(StaticValue::Float(150.0)));
        assert_eq!(
            parse_literal("2.h"),
            Some(StaticValue::Duration(Quantity {
                value: 2.0,
                factor: 3600e9
            }))
        );
        assert_eq!(
            parse_literal("-1.kib"),
            Some(StaticValue::DataSize(Quantity {
                value: -1.0,
                factor: 1024.0
            }))
        );
        assert_eq!(
            parse_literal(r##"#"raw\text"#"##),
            Some(StaticValue::String(r"raw\text".into()))
        );
        assert_eq!(
            parse_literal(r#""a\n\u{1f600}""#),
            Some(StaticValue::String("a\n😀".into()))
        );
        for unknown in [
            "NaN",
            "inf",
            "9223372036854775808",
            "foo",
            r##"#"a"# + #"b"#"##,
            "1 + 1",
            r#""hello \(name)""#,
        ] {
            assert_eq!(parse_literal(unknown), None, "{unknown}");
        }
    }
    #[test]
    fn constraint_numeric_predicates() {
        for (expr, literal, expected) in [
            ("this >= 0", "-1", Truth::False),
            ("this <= - 1", "-1", Truth::True),
            ("this.isPositive", "0", Truth::True),
            ("isPositive", "-0.1", Truth::False),
            ("isNonZero", "0", Truth::False),
            ("isEven", "-2", Truth::True),
            ("isOdd", "-3", Truth::True),
            ("isEven", "2.0", Truth::Undecidable),
            ("isBetween(0, 10)", "10", Truth::True),
            ("this.isBetween(0, 10)", "11", Truth::False),
            ("this < 2.s", "1500.ms", Truth::True),
            ("this == 1.kib", "1024.b", Truth::True),
            (
                "this == 9007199254740992.0",
                "9007199254740993",
                Truth::Undecidable,
            ),
        ] {
            assert_eq!(evaluate(expr, literal), expected, "{expr} / {literal}");
        }
    }
    #[test]
    fn constraint_quantity_runtime_semantics() {
        for (expr, literal, expected) in [
            ("this == 0.s", "-0.s", Truth::True),
            ("this < 0.s", "-0.s", Truth::True),
            ("this < -0.s", "0.s", Truth::False),
            ("isPositive", "-0.s", Truth::True),
            ("isNonZero", "-0.s", Truth::False),
            ("this == 0.b", "-0.b", Truth::True),
            ("this < 0.b", "-0.b", Truth::True),
            (
                "this < 0.09999999999999877.s",
                "0.09999999999999877.s",
                Truth::True,
            ),
            (
                "this == 0.09999999999999877.s",
                "0.09999999999999877.s",
                Truth::True,
            ),
            ("this > 1e308.s", "1e308.s", Truth::True),
            ("this == 1e308.s", "1e308.s", Truth::True),
            ("this < 0.0", "-0.0", Truth::False),
        ] {
            assert_eq!(evaluate(expr, literal), expected, "{expr} / {literal}");
        }
        assert_eq!(
            -Quantity {
                value: 0.0,
                factor: 1e9
            },
            Quantity {
                value: -0.0,
                factor: 1e9
            }
        );
        assert!(!implies(&["this == 0.s"], &["this >= 0.s"]));
        assert!(!implies(&["this > 1.s"], &["this > 500.ms"]));
    }
    #[test]
    fn constraint_boolean_logic() {
        assert_eq!(evaluate("isPositive && isEven", "2"), Truth::True);
        assert_eq!(evaluate("isPositive && isEven", "-2"), Truth::False);
        assert_eq!(evaluate("!isPositive || this == 2", "-1"), Truth::True);
        assert_eq!(
            evaluate("this > 0 && (this < 10 || this == 20)", "20"),
            Truth::True
        );
        assert_eq!(evaluate("false && custom()", "1"), Truth::False);
        assert_eq!(evaluate("true || custom()", "1"), Truth::True);
        assert_eq!(evaluate("!custom()", "1"), Truth::Undecidable);
        assert_eq!(evaluate("this == true", "true"), Truth::True);
    }
    #[test]
    fn constraint_strings_and_collections() {
        for (expr, literal, expected) in [
            ("length == 3", r#""abc""#, Truth::True),
            ("this.length == 1", r#""😀""#, Truth::True),
            ("isEmpty", r#""""#, Truth::True),
            (
                r#"startsWith("a") && endsWith("c") && contains("b")"#,
                r#""abc""#,
                Truth::True,
            ),
            (r##"matches(Regex(#"[a-z]+"#))"##, r#""abc""#, Truth::True),
            (r##"matches(Regex(#"[a-z]+"#))"##, r#""1abc""#, Truth::False),
            (
                r##"matches(Regex(#"(?i)abc"#))"##,
                r#""ABC""#,
                Truth::Undecidable,
            ),
            (r##"matches(Regex(#"["#))"##, r#""abc""#, Truth::Undecidable),
            (
                r##"matches(Regex(#"a++a"#))"##,
                r#""aa""#,
                Truth::Undecidable,
            ),
            (
                r##"matches(Regex(#"\w+"#))"##,
                r#""abc""#,
                Truth::Undecidable,
            ),
            (r##"matches(Regex(#"\d*"#))"##, r#""123""#, Truth::True),
            (r##"matches(Regex(#"\D+"#))"##, r#""abc""#, Truth::True),
            (
                r##"matches(Regex(#"[a--b]"#))"##,
                r#""a""#,
                Truth::Undecidable,
            ),
            (
                r##"matches(Regex(#"[a~~b]"#))"##,
                r#""a""#,
                Truth::Undecidable,
            ),
            (
                r##"matches(Regex(#"[a[b]]"#))"##,
                r#""a""#,
                Truth::Undecidable,
            ),
        ] {
            assert_eq!(evaluate(expr, literal), expected, "{expr}");
        }
        assert_eq!(
            ConstraintExpr::opaque("length > 1").evaluate(&StaticValue::CollectionLength(2)),
            Truth::True
        );
        assert_eq!(
            ConstraintExpr::opaque("isEmpty").evaluate(&StaticValue::CollectionLength(0)),
            Truth::True
        );
    }
    #[test]
    fn constraint_interval_proofs() {
        for (source, target, expected) in [
            (vec!["this > 10"], vec!["this > 0"], true),
            (vec!["this >= 0"], vec!["this > 0"], false),
            (vec!["this > 0"], vec!["this >= 0"], true),
            (
                vec!["this > 0 && this < 10"],
                vec!["isBetween(0, 10)"],
                true,
            ),
            (
                vec!["this >= 1", "this <= 9"],
                vec!["this > 0 && this < 10"],
                true,
            ),
            (vec!["1 < this"], vec!["isPositive"], true),
            (vec!["length >= 2"], vec!["length > 0"], true),
            (vec!["length > 0"], vec!["this > 0"], false),
            (vec!["isEmpty"], vec!["length == 0"], true),
            (vec!["this > 1.s"], vec!["this > 500.ms"], false),
            (vec!["this > 1.s"], vec!["this > 1.b"], false),
            (vec!["custom()"], vec!["this > 0"], false),
            (vec!["custom()"], vec!["custom()"], true),
            (vec!["foo + 1"], vec!["bar + 1"], false),
            (vec![], vec!["true"], true),
            (vec![], vec!["this > 0"], false),
        ] {
            assert_eq!(
                implies(&source, &target),
                expected,
                "{source:?} -> {target:?}"
            );
        }
    }
    #[test]
    fn constraint_implicit_member_detection() {
        for text in [
            "length > 0",
            "isPositive",
            "this.length > 0 && isEven",
            "foo.length > 0",
            "matches(Regex(\"a\"))",
        ] {
            assert!(
                ConstraintExpr::opaque(text).has_implicit_members(),
                "{text}"
            );
        }
        for text in [
            "this.length > 0",
            "this . isPositive",
            "this.isBetween(0, 10)",
            "this.matches(Regex(\"length\"))",
            "this == \"isPositive\"",
            "this > 0",
        ] {
            assert!(
                !ConstraintExpr::opaque(text).has_implicit_members(),
                "{text}"
            );
        }
    }
    #[test]
    fn constraint_preserves_source_and_bounds_recursion() {
        let source = " this >=  0 ";
        let expr = ConstraintExpr::opaque(source);
        assert_eq!(expr.to_string(), source);
        assert_eq!(expr, ConstraintExpr::opaque(source));
        let deep = format!("{}true{}", "(".repeat(200), ")".repeat(200));
        assert_eq!(
            ConstraintExpr::opaque(deep).evaluate(&StaticValue::Int(0)),
            Truth::Undecidable
        );
        for text in [
            "é",
            "!é",
            "this + 1",
            "this >=",
            "(",
            "startsWith(\"a\") trailing",
        ] {
            assert_eq!(
                ConstraintExpr::opaque(text).evaluate(&StaticValue::Int(0)),
                Truth::Undecidable
            );
        }
    }
}
