//! `RegExp.prototype.test` for a documented subset of
//! ECMAScript regular expressions (used by the zod string formats and
//! `.regex(/literal/)` checks, `docs/reference/zod.md`).
//!
//! The semantics are ECMAScript's *non-Unicode* mode (no `u`/`v` flag):
//! patterns and inputs are sequences of UTF-16 code units, a character
//! outside the Basic Multilingual Plane is two units, and matching is
//! backtracking with ECMAScript's rules (alternatives in order, greedy and
//! lazy quantifiers, an iteration that matches the empty string ends a
//! loop, lookaheads are atomic). Only `test` is modeled: whether a match
//! exists, never its captures.
//!
//! Supported syntax: literals, `.`, `^`, `$`, `\b`, `\B`, alternation,
//! capturing, non-capturing and named groups (captures are not observable
//! through `test`), lookaheads `(?=…)` / `(?!…)`, quantifiers `*`, `+`,
//! `?`, `{n}`, `{n,}`, `{n,m}` (greedy and lazy), character classes with
//! ranges and negation, the escapes `\d \D \w \W \s \S \t \n \v \f \r \0
//! \xHH \uHHHH \cX` and identity escapes of non-alphanumeric characters.
//! Flags: `i` (only for patterns whose literals and class members are
//! ASCII: then non-Unicode case-insensitive matching is ASCII case folding
//! exactly, since ECMAScript's `Canonicalize` never maps a non-ASCII unit to
//! an ASCII one), `m`, `s`, `g` and `y` (zod resets `lastIndex` to 0 before
//! every `test`, so `g` has no effect and `y` anchors the match at 0), `d`.
//! Everything else (backreferences, lookbehind, the `u`/`v` flags, octal
//! escapes, Annex B forms such as a literal `{` or `]`, an identity escape
//! of a letter) is refused by [`Regex::new`] with the construct named.

/// A compiled pattern.
#[derive(Clone, Debug)]
pub struct Regex {
    node: Node,
    ignore_case: bool,
    multiline: bool,
    dot_all: bool,
    sticky: bool,
}

#[derive(Clone, Debug)]
enum Node {
    Empty,
    Unit(u16),
    /// `.`
    Any,
    Class(Class),
    /// `^`
    Start,
    /// `$`
    End,
    WordBoundary(bool),
    Look {
        negative: bool,
        inner: Box<Node>,
    },
    Seq(Vec<Node>),
    Alt(Vec<Node>),
    Repeat {
        inner: Box<Node>,
        min: u32,
        max: Option<u32>,
        greedy: bool,
    },
}

#[derive(Clone, Debug)]
struct Class {
    negated: bool,
    items: Vec<ClassItem>,
}

#[derive(Clone, Copy, Debug)]
enum ClassItem {
    Range(u16, u16),
    Digit(bool),
    Word(bool),
    Space(bool),
}

/// The most backtracking steps one `test` may take; beyond it the result
/// is refused, never guessed.
const MAX_STEPS: u64 = 2_000_000;

fn is_line_terminator(u: u16) -> bool {
    matches!(u, 0x0A | 0x0D | 0x2028 | 0x2029)
}

fn is_word(u: u16) -> bool {
    matches!(u, 0x30..=0x39 | 0x41..=0x5A | 0x61..=0x7A | 0x5F)
}

fn is_space(u: u16) -> bool {
    char::from_u32(u32::from(u)).is_some_and(crate::jsfn::is_js_whitespace)
}

fn ascii_fold(u: u16) -> [u16; 3] {
    let lower = if (0x41..=0x5A).contains(&u) {
        u + 32
    } else {
        u
    };
    let upper = if (0x61..=0x7A).contains(&u) {
        u - 32
    } else {
        u
    };
    [u, lower, upper]
}

impl ClassItem {
    fn contains(self, u: u16) -> bool {
        match self {
            Self::Range(lo, hi) => lo <= u && u <= hi,
            Self::Digit(neg) => (0x30..=0x39).contains(&u) != neg,
            Self::Word(neg) => is_word(u) != neg,
            Self::Space(neg) => is_space(u) != neg,
        }
    }
}

impl Class {
    fn matches(&self, u: u16, ignore_case: bool) -> bool {
        let hit = |c: u16| self.items.iter().any(|i| i.contains(c));
        // Non-Unicode `i`: a member matches when its canonical (upper-case)
        // form equals the input's; with ASCII members only, that is ASCII
        // case folding (a non-ASCII unit never canonicalizes to ASCII).
        // `\w`, `\d`, `\s` and their complements are not case-folded sets
        // in non-Unicode mode, but their membership is closed under ASCII
        // case folding, so folding them is harmless.
        let found = if ignore_case && u < 128 {
            ascii_fold(u).into_iter().any(hit)
        } else {
            hit(u)
        };
        found != self.negated
    }
}

struct Parser<'p> {
    units: &'p [u16],
    pos: usize,
    ascii_only: bool,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u16> {
        self.units.get(self.pos).copied()
    }
    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c as u16) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn literal(&mut self, u: u16) -> u16 {
        if u >= 128 {
            self.ascii_only = false;
        }
        u
    }

    fn disjunction(&mut self) -> Result<Node, String> {
        let mut alts = vec![self.alternative()?];
        while self.eat('|') {
            alts.push(self.alternative()?);
        }
        Ok(if alts.len() == 1 {
            alts.pop().unwrap_or(Node::Empty)
        } else {
            Node::Alt(alts)
        })
    }

    fn alternative(&mut self) -> Result<Node, String> {
        let mut items = Vec::new();
        while let Some(u) = self.peek() {
            if u == '|' as u16 || u == ')' as u16 {
                break;
            }
            let atom = self.term()?;
            items.push(atom);
        }
        Ok(match items.len() {
            0 => Node::Empty,
            1 => items.pop().unwrap_or(Node::Empty),
            _ => Node::Seq(items),
        })
    }

    fn term(&mut self) -> Result<Node, String> {
        let Some(u) = self.peek() else {
            return Err("an unexpected end of the pattern".to_owned());
        };
        self.pos += 1;
        let (atom, quantifiable) = match char::from_u32(u32::from(u)).unwrap_or('\u{FFFD}') {
            '^' => (Node::Start, false),
            '$' => (Node::End, false),
            '.' => (Node::Any, true),
            '(' => {
                if self.eat('?') {
                    if self.eat(':') {
                        let inner = self.disjunction()?;
                        self.close_group()?;
                        (inner, true)
                    } else if self.eat('=') || self.eat('!') {
                        let negative = self.units[self.pos - 1] == '!' as u16;
                        let inner = self.disjunction()?;
                        self.close_group()?;
                        (
                            Node::Look {
                                negative,
                                inner: Box::new(inner),
                            },
                            false,
                        )
                    } else if self.eat('<') {
                        if self.peek() == Some('=' as u16) || self.peek() == Some('!' as u16) {
                            return Err("a lookbehind assertion".to_owned());
                        }
                        // A named group: its name, then the group.
                        while let Some(c) = self.peek() {
                            self.pos += 1;
                            if c == '>' as u16 {
                                break;
                            }
                            if !(is_word(c) || c == '$' as u16) {
                                return Err("a named group with an unusual name".to_owned());
                            }
                        }
                        let inner = self.disjunction()?;
                        self.close_group()?;
                        (inner, true)
                    } else {
                        return Err("a group modifier `(?…)`".to_owned());
                    }
                } else {
                    let inner = self.disjunction()?;
                    self.close_group()?;
                    (inner, true)
                }
            }
            '[' => (Node::Class(self.class()?), true),
            '\\' => self.escape()?,
            '*' | '+' | '?' => return Err("a quantifier with nothing to repeat".to_owned()),
            '{' => return Err("a literal `{` (an Annex B form)".to_owned()),
            '}' => return Err("a literal `}` (an Annex B form)".to_owned()),
            ']' => return Err("a literal `]` (an Annex B form)".to_owned()),
            _ => (Node::Unit(self.literal(u)), true),
        };
        let Some((min, max)) = self.quantifier()? else {
            return Ok(atom);
        };
        if !quantifiable {
            return Err("a quantified assertion".to_owned());
        }
        let greedy = !self.eat('?');
        Ok(Node::Repeat {
            inner: Box::new(atom),
            min,
            max,
            greedy,
        })
    }

    fn close_group(&mut self) -> Result<(), String> {
        if self.eat(')') {
            Ok(())
        } else {
            Err("an unterminated group".to_owned())
        }
    }

    fn number(&mut self) -> Option<u32> {
        let start = self.pos;
        let mut n: u32 = 0;
        while let Some(u) = self.peek()
            && (0x30..=0x39).contains(&u)
        {
            n = n.checked_mul(10)?.checked_add(u32::from(u - 0x30))?;
            self.pos += 1;
        }
        (self.pos > start).then_some(n)
    }

    fn quantifier(&mut self) -> Result<Option<(u32, Option<u32>)>, String> {
        let Some(u) = self.peek() else {
            return Ok(None);
        };
        let q = match char::from_u32(u32::from(u)).unwrap_or('\u{FFFD}') {
            '*' => (0, None),
            '+' => (1, None),
            '?' => (0, Some(1)),
            '{' => {
                self.pos += 1;
                let Some(min) = self.number() else {
                    return Err("a literal `{` (an Annex B form)".to_owned());
                };
                let max = if self.eat(',') {
                    if self.peek() == Some('}' as u16) {
                        None
                    } else {
                        Some(
                            self.number()
                                .ok_or_else(|| "a malformed `{n,m}` quantifier".to_owned())?,
                        )
                    }
                } else {
                    Some(min)
                };
                if !self.eat('}') {
                    return Err("a malformed `{n,m}` quantifier".to_owned());
                }
                if max.is_some_and(|m| m < min) {
                    return Err("a `{n,m}` quantifier with m < n (a SyntaxError)".to_owned());
                }
                return Ok(Some((min, max)));
            }
            _ => return Ok(None),
        };
        self.pos += 1;
        Ok(Some(q))
    }

    fn hex(&mut self, n: usize) -> Result<u16, String> {
        let mut v: u16 = 0;
        for _ in 0..n {
            let d = self
                .peek()
                .and_then(|u| char::from_u32(u32::from(u)))
                .and_then(|c| c.to_digit(16))
                .ok_or_else(|| "a malformed hexadecimal escape".to_owned())?;
            v = v * 16 + d as u16;
            self.pos += 1;
        }
        Ok(v)
    }

    /// A character escape (after `\`) that denotes one code unit, shared by
    /// atoms and classes; `None` for class escapes and assertions.
    fn unit_escape(&mut self, c: char, in_class: bool) -> Result<Option<u16>, String> {
        Ok(Some(match c {
            't' => 0x09,
            'n' => 0x0A,
            'v' => 0x0B,
            'f' => 0x0C,
            'r' => 0x0D,
            'b' if in_class => 0x08,
            '0' => {
                if self.peek().is_some_and(|u| (0x30..=0x39).contains(&u)) {
                    return Err("an octal escape".to_owned());
                }
                0
            }
            'x' => {
                let v = self.hex(2)?;
                self.literal(v)
            }
            'u' => {
                let v = self.hex(4)?;
                self.literal(v)
            }
            'c' => {
                let letter = self
                    .peek()
                    .filter(|u| (0x41..=0x5A).contains(u) || (0x61..=0x7A).contains(u))
                    .ok_or_else(|| "a `\\c` escape without a letter".to_owned())?;
                self.pos += 1;
                letter % 32
            }
            '1'..='9' => return Err("a backreference (or an octal escape)".to_owned()),
            'k' => return Err("a named backreference".to_owned()),
            'd' | 'D' | 'w' | 'W' | 's' | 'S' => return Ok(None),
            'b' | 'B' => return Ok(None),
            c if c.is_ascii_alphanumeric() => {
                return Err(format!("the identity escape `\\{c}` (an Annex B form)"));
            }
            c if (c as u32) < 0x10000 => self.literal(c as u16),
            _ => return Err("an escape of a non-BMP character".to_owned()),
        }))
    }

    fn class_escape(c: char) -> Option<ClassItem> {
        Some(match c {
            'd' => ClassItem::Digit(false),
            'D' => ClassItem::Digit(true),
            'w' => ClassItem::Word(false),
            'W' => ClassItem::Word(true),
            's' => ClassItem::Space(false),
            'S' => ClassItem::Space(true),
            _ => return None,
        })
    }

    fn escape(&mut self) -> Result<(Node, bool), String> {
        let u = self
            .peek()
            .ok_or_else(|| "a `\\` at the end of the pattern".to_owned())?;
        self.pos += 1;
        let c = char::from_u32(u32::from(u)).unwrap_or('\u{FFFD}');
        match c {
            'b' => return Ok((Node::WordBoundary(true), false)),
            'B' => return Ok((Node::WordBoundary(false), false)),
            _ => {}
        }
        if let Some(item) = Self::class_escape(c) {
            return Ok((
                Node::Class(Class {
                    negated: false,
                    items: vec![item],
                }),
                true,
            ));
        }
        match self.unit_escape(c, false)? {
            Some(v) => Ok((Node::Unit(v), true)),
            None => Err(format!("the escape `\\{c}`")),
        }
    }

    /// One class atom: a code unit, or a class escape.
    fn class_atom(&mut self) -> Result<Result<u16, ClassItem>, String> {
        let u = self
            .peek()
            .ok_or_else(|| "an unterminated character class".to_owned())?;
        self.pos += 1;
        if u != '\\' as u16 {
            return Ok(Ok(self.literal(u)));
        }
        let e = self
            .peek()
            .ok_or_else(|| "a `\\` at the end of the pattern".to_owned())?;
        self.pos += 1;
        let c = char::from_u32(u32::from(e)).unwrap_or('\u{FFFD}');
        if let Some(item) = Self::class_escape(c) {
            return Ok(Err(item));
        }
        if c == '-' {
            return Ok(Ok('-' as u16));
        }
        match self.unit_escape(c, true)? {
            Some(v) => Ok(Ok(v)),
            None => Err(format!("the class escape `\\{c}`")),
        }
    }

    fn class(&mut self) -> Result<Class, String> {
        let negated = self.eat('^');
        let mut items = Vec::new();
        loop {
            match self.peek() {
                None => return Err("an unterminated character class".to_owned()),
                Some(u) if u == ']' as u16 => {
                    self.pos += 1;
                    break;
                }
                _ => {}
            }
            let first = self.class_atom()?;
            let is_range = self.peek() == Some('-' as u16)
                && self
                    .units
                    .get(self.pos + 1)
                    .is_some_and(|u| *u != ']' as u16);
            if is_range {
                self.pos += 1;
                let second = self.class_atom()?;
                match (first, second) {
                    (Ok(lo), Ok(hi)) if lo <= hi => items.push(ClassItem::Range(lo, hi)),
                    (Ok(_), Ok(_)) => {
                        return Err("a class range out of order (a SyntaxError)".to_owned());
                    }
                    _ => {
                        return Err(
                            "a class range with a class escape (an Annex B form)".to_owned()
                        );
                    }
                }
            } else {
                items.push(match first {
                    Ok(u) => ClassItem::Range(u, u),
                    Err(item) => item,
                });
            }
        }
        Ok(Class { negated, items })
    }
}

impl Regex {
    /// Compile `pattern` (the source between the slashes) with `flags`, or
    /// say which construct is not supported.
    pub fn new(pattern: &str, flags: &str) -> Result<Self, String> {
        let mut seen = String::new();
        let (mut ignore_case, mut multiline, mut dot_all, mut sticky) =
            (false, false, false, false);
        for f in flags.chars() {
            if seen.contains(f) {
                return Err(format!("the repeated flag `{f}` (a SyntaxError)"));
            }
            seen.push(f);
            match f {
                'i' => ignore_case = true,
                'm' => multiline = true,
                's' => dot_all = true,
                'y' => sticky = true,
                'g' | 'd' => {}
                'u' | 'v' => {
                    return Err(format!(
                        "the `{f}` flag (Unicode-mode regular expressions are not modeled)"
                    ));
                }
                other => return Err(format!("the flag `{other}`")),
            }
        }
        let units: Vec<u16> = pattern.encode_utf16().collect();
        let mut p = Parser {
            units: &units,
            pos: 0,
            ascii_only: true,
        };
        let node = p.disjunction()?;
        if p.pos != units.len() {
            return Err("an unmatched `)`".to_owned());
        }
        if ignore_case && !p.ascii_only {
            return Err(
                "the `i` flag with non-ASCII characters in the pattern (Unicode case folding is not modeled)"
                    .to_owned(),
            );
        }
        Ok(Self {
            node,
            ignore_case,
            multiline,
            dot_all,
            sticky,
        })
    }

    /// A short string the pattern matches (for input synthesis): the first
    /// alternative, the minimum of every quantifier, the first member of
    /// every class; assertions are ignored while building and the result is
    /// checked with [`Regex::test`]. `None` when that guess does not match.
    pub fn example(&self) -> Option<String> {
        fn build(n: &Node, out: &mut Vec<u16>) -> Option<()> {
            match n {
                Node::Empty
                | Node::Start
                | Node::End
                | Node::WordBoundary(_)
                | Node::Look { .. } => {}
                Node::Unit(u) => out.push(*u),
                Node::Any => out.push(u16::from(b'a')),
                Node::Class(c) => {
                    let pick = (0x20u16..0x7F)
                        .chain([0x0A, 0x09])
                        .find(|u| c.matches(*u, false))?;
                    // Prefer a letter or digit when the class has one.
                    let nicer = (u16::from(b'a')..=u16::from(b'z'))
                        .chain(u16::from(b'0')..=u16::from(b'9'))
                        .find(|u| c.matches(*u, false));
                    out.push(nicer.unwrap_or(pick));
                }
                Node::Seq(items) => {
                    for i in items {
                        build(i, out)?;
                    }
                }
                Node::Alt(alts) => build(alts.first()?, out)?,
                Node::Repeat { inner, min, .. } => {
                    for _ in 0..(*min).min(64) {
                        build(inner, out)?;
                    }
                }
            }
            Some(())
        }
        let mut units = Vec::new();
        build(&self.node, &mut units)?;
        let s = String::from_utf16(&units).ok()?;
        self.test(&s).ok()?.then_some(s)
    }

    /// `regex.test(input)` with `lastIndex` 0, or why the answer is not
    /// known (the backtracking bound).
    pub fn test(&self, input: &str) -> Result<bool, String> {
        let units: Vec<u16> = input.encode_utf16().collect();
        let m = Matcher {
            re: self,
            input: &units,
            steps: std::cell::Cell::new(0),
        };
        let last = if self.sticky { 0 } else { units.len() };
        for start in 0..=last {
            if m.node(&self.node, start, &mut |_| Ok(true))? {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

struct Matcher<'r> {
    re: &'r Regex,
    input: &'r [u16],
    steps: std::cell::Cell<u64>,
}

type Cont<'c> = dyn FnMut(usize) -> Result<bool, String> + 'c;

impl Matcher<'_> {
    fn tick(&self) -> Result<(), String> {
        self.steps.set(self.steps.get() + 1);
        if self.steps.get() > MAX_STEPS {
            return Err(format!(
                "a regular expression test that backtracks more than {MAX_STEPS} steps"
            ));
        }
        Ok(())
    }

    fn unit_matches(&self, n: &Node, u: u16) -> bool {
        match n {
            Node::Unit(c) => {
                if self.re.ignore_case && u < 128 && *c < 128 {
                    ascii_fold(u).contains(c)
                } else {
                    *c == u
                }
            }
            Node::Any => self.re.dot_all || !is_line_terminator(u),
            Node::Class(class) => class.matches(u, self.re.ignore_case),
            _ => false,
        }
    }

    fn node(&self, n: &Node, i: usize, k: &mut Cont<'_>) -> Result<bool, String> {
        self.tick()?;
        match n {
            Node::Empty => k(i),
            Node::Unit(_) | Node::Any | Node::Class(_) => match self.input.get(i) {
                Some(&u) if self.unit_matches(n, u) => k(i + 1),
                _ => Ok(false),
            },
            Node::Start => {
                let ok = i == 0 || (self.re.multiline && is_line_terminator(self.input[i - 1]));
                if ok { k(i) } else { Ok(false) }
            }
            Node::End => {
                let ok = i == self.input.len()
                    || (self.re.multiline && is_line_terminator(self.input[i]));
                if ok { k(i) } else { Ok(false) }
            }
            Node::WordBoundary(want) => {
                let before = i > 0 && is_word(self.input[i - 1]);
                let after = i < self.input.len() && is_word(self.input[i]);
                if (before != after) == *want {
                    k(i)
                } else {
                    Ok(false)
                }
            }
            Node::Look { negative, inner } => {
                // Lookaheads are atomic: once matched, not backtracked into.
                let found = self.node(inner, i, &mut |_| Ok(true))?;
                if found != *negative { k(i) } else { Ok(false) }
            }
            Node::Seq(items) => self.seq(items, i, k),
            Node::Alt(alts) => {
                for a in alts {
                    if self.node(a, i, k)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Node::Repeat {
                inner,
                min,
                max,
                greedy,
            } => self.repeat(inner, (*min, *max, *greedy), 0, i, k),
        }
    }

    fn seq(&self, items: &[Node], i: usize, k: &mut Cont<'_>) -> Result<bool, String> {
        match items.split_first() {
            None => k(i),
            Some((first, rest)) => self.node(first, i, &mut |j| self.seq(rest, j, k)),
        }
    }

    fn repeat(
        &self,
        inner: &Node,
        q: (u32, Option<u32>, bool),
        count: u32,
        i: usize,
        k: &mut Cont<'_>,
    ) -> Result<bool, String> {
        self.tick()?;
        let (min, max, greedy) = q;
        let can_stop = count >= min;
        let can_go = max.is_none_or(|m| count < m);
        if !greedy && can_stop && k(i)? {
            return Ok(true);
        }
        if can_go {
            // ECMAScript RepeatMatcher: once the minimum is met, an
            // iteration that consumes nothing fails.
            let found = self.node(inner, i, &mut |j| {
                if j == i && count >= min {
                    return Ok(false);
                }
                self.repeat(inner, q, count.saturating_add(1), j, k)
            })?;
            if found {
                return Ok(true);
            }
        }
        if greedy && can_stop {
            return k(i);
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::Regex;

    fn t(p: &str, f: &str, s: &str) -> bool {
        Regex::new(p, f).unwrap().test(s).unwrap()
    }

    #[test]
    fn basics_follow_ecmascript() {
        assert!(t("^a+b$", "", "aaab"));
        assert!(!t("^a+b$", "", "aaa"));
        assert!(t("colou?r", "", "the color"));
        assert!(t("^(?:ab|a)c$", "", "ac"));
        assert!(t("^[a-c]{2,3}$", "", "abc"));
        assert!(!t("^[a-c]{2,3}$", "", "abcd"));
        assert!(t("^[^0-9]+$", "", "abc"));
        assert!(t("^\\d\\w\\s$", "", "1a "));
        assert!(t("^(?!\\.)[a-z.]+$", "", "a.b"));
        assert!(!t("^(?!\\.)[a-z.]+$", "", ".ab"));
        assert!(t("^ABC$", "i", "abc"));
        assert!(!t("^.$", "", "\n"));
        assert!(t("^.$", "s", "\n"));
        assert!(t("^b$", "m", "a\nb"));
        // Non-Unicode mode: a non-BMP character is two code units.
        assert!(!t("^.$", "", "\u{1F600}"));
        assert!(t("^..$", "", "\u{1F600}"));
        // An empty iteration ends a loop.
        assert!(t("^(a*)*$", "", "aaa"));
        // `y` anchors at lastIndex 0.
        assert!(!t("b", "y", "ab"));
        assert!(t("\\bfoo\\b", "", "a foo b"));
        assert!(!t("\\bfoo\\b", "", "afoo"));
    }

    #[test]
    fn unsupported_syntax_is_refused_by_name() {
        for (p, f) in [
            ("(a)\\1", ""),
            ("(?<=a)b", ""),
            ("a", "u"),
            ("é", "i"),
            ("a{", ""),
            ("\\a", ""),
            ("\\012", ""),
        ] {
            assert!(Regex::new(p, f).is_err(), "{p}/{f}");
        }
    }
}
