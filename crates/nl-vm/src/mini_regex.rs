//! Minimal backtracking regex engine, used only by
//! `system.io.File.glob` (stdlib.md § system.io.File (glob)). The workspace
//! has no regex dependency (same "no external crate" stance as
//! `system.SecureRandom`/`system.Uuid`, which read `/dev/urandom` directly
//! rather than pulling in `getrandom`), so a small hand-rolled matcher is
//! used instead of a real one.
//!
//! Supported syntax: literal characters, `.` (any char), `*`/`+`/`?`
//! (greedy quantifiers on the previous atom), character classes (`[abc]`,
//! `[^abc]`, `[a-z]`), grouping `(...)`, alternation `|`, and backslash
//! escapes (`\d`/`\w`/`\s`, their uppercase negations, or `\X` for a literal
//! `X` — covers stdlib.md's own example pattern `.*\.nl`, written in NL
//! source as the string literal `".*\\.nl"`). Not supported: counted
//! repetition (`{m,n}`), anchors (`^`/`$` — unnecessary since `is_match`
//! always matches the *whole* string, never searches for a substring),
//! backreferences. `glob`-style `*`/`**`/`?` wildcards are a different,
//! unsupported syntax — stdlib.md documents `system.io.File.glob`'s
//! `pattern` as glob-or-regex, but its own worked example
//! (`system.io.File.glob("src", ".*\\.nl")`) is a regex, so that's the
//! syntax implemented here; a caller wanting glob wildcards must spell them
//! out as regex (`*.txt` -> `[^/]*\.txt`).

pub struct Regex {
    root: Node,
}

#[derive(Debug, Clone)]
enum Node {
    Char(char),
    Any,
    Class(Vec<(char, char)>, bool),
    Concat(Vec<Node>),
    Alt(Vec<Node>),
    Star(Box<Node>),
    Plus(Box<Node>),
    Opt(Box<Node>),
}

impl Regex {
    pub fn compile(pattern: &str) -> Result<Regex, String> {
        let mut parser = Parser { chars: pattern.chars().collect(), pos: 0 };
        let root = parser.parse_alt()?;
        if parser.pos != parser.chars.len() {
            return Err(format!("unexpected character at position {}", parser.pos));
        }
        Ok(Regex { root })
    }

    /// Whole-string match (matching stdlib.md: "Matching is applied to the
    /// relative path"), not a substring search.
    pub fn is_match(&self, s: &str) -> bool {
        let chars: Vec<char> = s.chars().collect();
        match_node(&self.root, &chars, &|rest| rest.is_empty())
    }
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn parse_alt(&mut self) -> Result<Node, String> {
        let mut branches = vec![self.parse_concat()?];
        while self.peek() == Some('|') {
            self.bump();
            branches.push(self.parse_concat()?);
        }
        Ok(if branches.len() == 1 { branches.pop().expect("just pushed") } else { Node::Alt(branches) })
    }

    fn parse_concat(&mut self) -> Result<Node, String> {
        let mut items = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            items.push(self.parse_postfix()?);
        }
        Ok(Node::Concat(items))
    }

    fn parse_postfix(&mut self) -> Result<Node, String> {
        let atom = self.parse_atom()?;
        match self.peek() {
            Some('*') => {
                self.bump();
                Ok(Node::Star(Box::new(atom)))
            }
            Some('+') => {
                self.bump();
                Ok(Node::Plus(Box::new(atom)))
            }
            Some('?') => {
                self.bump();
                Ok(Node::Opt(Box::new(atom)))
            }
            _ => Ok(atom),
        }
    }

    fn parse_atom(&mut self) -> Result<Node, String> {
        match self.bump() {
            Some('.') => Ok(Node::Any),
            Some('(') => {
                let inner = self.parse_alt()?;
                if self.bump() != Some(')') {
                    return Err("expected ')'".to_string());
                }
                Ok(inner)
            }
            Some('[') => self.parse_class(),
            Some('\\') => {
                let esc = self.bump().ok_or("dangling escape at end of pattern")?;
                Ok(escape_node(esc))
            }
            Some(c) => Ok(Node::Char(c)),
            None => Err("unexpected end of pattern".to_string()),
        }
    }

    fn parse_class(&mut self) -> Result<Node, String> {
        let negated = if self.peek() == Some('^') {
            self.bump();
            true
        } else {
            false
        };
        let mut ranges = Vec::new();
        let mut first = true;
        loop {
            match self.peek() {
                None => return Err("unterminated character class".to_string()),
                Some(']') if !first => {
                    self.bump();
                    break;
                }
                _ => {
                    first = false;
                    let lo = self.bump_class_char()?;
                    if self.peek() == Some('-') && self.chars.get(self.pos + 1).is_some_and(|&c| c != ']') {
                        self.bump();
                        let hi = self.bump_class_char()?;
                        ranges.push((lo, hi));
                    } else {
                        ranges.push((lo, lo));
                    }
                }
            }
        }
        Ok(Node::Class(ranges, negated))
    }

    fn bump_class_char(&mut self) -> Result<char, String> {
        match self.bump() {
            Some('\\') => self.bump().ok_or_else(|| "dangling escape in character class".to_string()),
            Some(c) => Ok(c),
            None => Err("unterminated character class".to_string()),
        }
    }
}

fn escape_node(c: char) -> Node {
    const DIGIT: (char, char) = ('0', '9');
    const WORD: [(char, char); 4] = [('a', 'z'), ('A', 'Z'), ('0', '9'), ('_', '_')];
    const SPACE: [(char, char); 4] = [(' ', ' '), ('\t', '\t'), ('\n', '\n'), ('\r', '\r')];
    match c {
        'd' => Node::Class(vec![DIGIT], false),
        'D' => Node::Class(vec![DIGIT], true),
        'w' => Node::Class(WORD.to_vec(), false),
        'W' => Node::Class(WORD.to_vec(), true),
        's' => Node::Class(SPACE.to_vec(), false),
        'S' => Node::Class(SPACE.to_vec(), true),
        other => Node::Char(other),
    }
}

/// Backtracking match with an explicit success continuation `k` — lets
/// concatenation and quantifiers try alternatives without an explicit
/// choice-point stack.
fn match_node(node: &Node, s: &[char], k: &dyn Fn(&[char]) -> bool) -> bool {
    match node {
        Node::Char(c) => s.first() == Some(c) && k(&s[1..]),
        Node::Any => !s.is_empty() && k(&s[1..]),
        Node::Class(ranges, negated) => match s.first() {
            Some(&c) => {
                let inside = ranges.iter().any(|&(lo, hi)| c >= lo && c <= hi);
                (inside != *negated) && k(&s[1..])
            }
            None => false,
        },
        Node::Concat(items) => match_concat(items, s, k),
        Node::Alt(branches) => branches.iter().any(|b| match_node(b, s, k)),
        Node::Star(inner) => match_star(inner, s, k),
        Node::Plus(inner) => match_node(inner, s, &|s2| match_star(inner, s2, k)),
        Node::Opt(inner) => match_node(inner, s, k) || k(s),
    }
}

fn match_concat(items: &[Node], s: &[char], k: &dyn Fn(&[char]) -> bool) -> bool {
    match items.split_first() {
        None => k(s),
        Some((first, rest)) => match_node(first, s, &|s2| match_concat(rest, s2, k)),
    }
}

/// Greedy `*`: try consuming one more repetition before falling back to the
/// continuation. Guards against infinite recursion on a zero-width
/// repetition (e.g. a pathological `(a?)*`) by requiring every repetition to
/// strictly shrink the remaining input.
fn match_star(inner: &Node, s: &[char], k: &dyn Fn(&[char]) -> bool) -> bool {
    let len = s.len();
    match_node(inner, s, &|s2| s2.len() < len && match_star(inner, s2, k)) || k(s)
}

#[cfg(test)]
mod tests {
    use super::Regex;

    fn is_match(pattern: &str, s: &str) -> bool {
        Regex::compile(pattern).expect("valid pattern").is_match(s)
    }

    #[test]
    fn literal_and_dot() {
        assert!(is_match("abc", "abc"));
        assert!(!is_match("abc", "abd"));
        assert!(is_match("a.c", "abc"));
    }

    #[test]
    fn stdlib_example_pattern() {
        assert!(is_match(".*\\.nl", "src/main.nl"));
        assert!(!is_match(".*\\.nl", "src/main.txt"));
        assert!(is_match(".*", "anything at all"));
    }

    #[test]
    fn quantifiers_and_classes() {
        assert!(is_match("[a-z]+\\.txt", "readme.txt"));
        assert!(!is_match("[a-z]+\\.txt", "README.txt"));
        assert!(is_match("colou?r", "color"));
        assert!(is_match("colou?r", "colour"));
    }

    #[test]
    fn alternation_and_groups() {
        assert!(is_match("(foo|bar)\\.nl", "foo.nl"));
        assert!(is_match("(foo|bar)\\.nl", "bar.nl"));
        assert!(!is_match("(foo|bar)\\.nl", "baz.nl"));
    }
}
