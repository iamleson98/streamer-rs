//! Formulas typed on the board (`a*(x-h)^2+k`, `sin(2x)`, `v*cos(t)`), turned into functions: a
//! port of `board/math/expression.ts`, with the same grammar, limits, error kinds and positions.
//!
//! Numbers, the independent variable, single-letter parameters, `+ - * / ^`, parentheses and a
//! fixed list of functions. Typing is forgiving the way a teacher writes: `2x` is `2*x`, `a(x-h)`
//! is a times the bracket, `ax` is `a*x`, `sin x` is `sin(x)` and `π` is `pi`. `compile` never
//! panics: a formula that cannot be read is an error with its position (in UTF-16 units, as the
//! web player counts).

use std::collections::BTreeMap;
use std::f64::consts::{E, PI};

use super::{js_max, js_min, js_round};

pub const MAX_EXPRESSION_LENGTH: usize = 200;
const MAX_NODES: usize = 120;
const MAX_DEPTH: usize = 24;

/// Why a formula could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpressionError {
    Empty,
    TooLong,
    TooComplex,
    Unexpected,
    Unclosed,
}

/// An unreadable formula and where it went wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Failure {
    pub error: ExpressionError,
    pub at: usize,
}

/// What a formula's letters (besides its variable) are worth.
pub type Params = BTreeMap<char, f64>;

/// The independent variable: `x` for y = f(x), `t` for a parametric curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variable {
    X,
    T,
}

impl Variable {
    fn letter(self) -> char {
        match self {
            Self::X => 'x',
            Self::T => 't',
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Func {
    Sin,
    Cos,
    Tan,
    Cot,
    Sec,
    Csc,
    Asin,
    Acos,
    Atan,
    Sinh,
    Cosh,
    Tanh,
    Sqrt,
    Cbrt,
    Abs,
    Exp,
    Ln,
    Log,
    Floor,
    Ceil,
    Round,
    Sign,
    Min,
    Max,
}

/// The names a formula may call (letters only, as a name is a run of letters; the web player's
/// table also has `log2` and `atan2`, which can never be typed that way), longest match first.
const FUNCTIONS: [(&str, Func, usize); 30] = [
    ("sin", Func::Sin, 1),
    ("cos", Func::Cos, 1),
    ("tan", Func::Tan, 1),
    ("cot", Func::Cot, 1),
    ("sec", Func::Sec, 1),
    ("csc", Func::Csc, 1),
    ("asin", Func::Asin, 1),
    ("acos", Func::Acos, 1),
    ("atan", Func::Atan, 1),
    ("sinh", Func::Sinh, 1),
    ("cosh", Func::Cosh, 1),
    ("tanh", Func::Tanh, 1),
    ("sqrt", Func::Sqrt, 1),
    ("cbrt", Func::Cbrt, 1),
    ("abs", Func::Abs, 1),
    ("exp", Func::Exp, 1),
    ("ln", Func::Ln, 1),
    ("log", Func::Log, 1),
    ("floor", Func::Floor, 1),
    ("ceil", Func::Ceil, 1),
    ("round", Func::Round, 1),
    ("sign", Func::Sign, 1),
    ("min", Func::Min, 2),
    ("max", Func::Max, 2),
    ("arcsin", Func::Asin, 1),
    ("arccos", Func::Acos, 1),
    ("arctan", Func::Atan, 1),
    ("tg", Func::Tan, 1),
    ("cotg", Func::Cot, 1),
    ("lg", Func::Log, 1),
];

/// `Math.sign`.
fn js_sign(x: f64) -> f64 {
    if x.is_nan() || x == 0.0 {
        x
    } else {
        x.signum()
    }
}

/// `Math.pow`: unlike `powf`, a NaN exponent and a base of 1 or -1 to an infinite power are NaN.
fn js_pow(x: f64, y: f64) -> f64 {
    if y.is_nan() || (y.is_infinite() && x.abs() == 1.0) {
        f64::NAN
    } else {
        x.powf(y)
    }
}

fn call1(f: Func, degrees: bool, x: f64) -> f64 {
    let rad = |g: fn(f64) -> f64| g(if degrees { x * PI / 180.0 } else { x });
    let arc = |g: fn(f64) -> f64| if degrees { g(x) * 180.0 / PI } else { g(x) };
    match f {
        Func::Sin => rad(f64::sin),
        Func::Cos => rad(f64::cos),
        Func::Tan => rad(f64::tan),
        Func::Cot => rad(|v| 1.0 / v.tan()),
        Func::Sec => rad(|v| 1.0 / v.cos()),
        Func::Csc => rad(|v| 1.0 / v.sin()),
        Func::Asin => arc(f64::asin),
        Func::Acos => arc(f64::acos),
        Func::Atan => arc(f64::atan),
        Func::Sinh => x.sinh(),
        Func::Cosh => x.cosh(),
        Func::Tanh => x.tanh(),
        Func::Sqrt => x.sqrt(),
        Func::Cbrt => x.cbrt(),
        Func::Abs => x.abs(),
        Func::Exp => x.exp(),
        Func::Ln => x.ln(),
        Func::Log => x.log10(),
        Func::Floor => x.floor(),
        Func::Ceil => x.ceil(),
        Func::Round => js_round(x),
        Func::Sign => js_sign(x),
        Func::Min | Func::Max => unreachable!("two arguments"),
    }
}

#[derive(Debug, Clone)]
enum Node {
    Num(f64),
    Variable,
    Param(char),
    Neg(Box<Node>),
    Add(Box<Node>, Box<Node>),
    Sub(Box<Node>, Box<Node>),
    Mul(Box<Node>, Box<Node>),
    Div(Box<Node>, Box<Node>),
    Pow(Box<Node>, Box<Node>),
    Call1(Func, bool, Box<Node>),
    Call2(Func, Box<Node>, Box<Node>),
}

impl Node {
    fn eval(&self, v: f64, p: &Params) -> f64 {
        match self {
            Self::Num(n) => *n,
            Self::Variable => v,
            Self::Param(c) => p.get(c).copied().unwrap_or(1.0),
            Self::Neg(a) => -a.eval(v, p),
            Self::Add(a, b) => a.eval(v, p) + b.eval(v, p),
            Self::Sub(a, b) => a.eval(v, p) - b.eval(v, p),
            Self::Mul(a, b) => a.eval(v, p) * b.eval(v, p),
            Self::Div(a, b) => a.eval(v, p) / b.eval(v, p),
            Self::Pow(a, b) => js_pow(a.eval(v, p), b.eval(v, p)),
            Self::Call1(f, d, a) => call1(*f, *d, a.eval(v, p)),
            Self::Call2(f, a, b) => {
                if *f == Func::Min {
                    js_min(a.eval(v, p), b.eval(v, p))
                } else {
                    js_max(a.eval(v, p), b.eval(v, p))
                }
            }
        }
    }
}

/// A formula that was read.
#[derive(Debug, Clone)]
pub struct Expression {
    /// The parameters the formula uses, in order of appearance.
    pub params: Vec<char>,
    root: Node,
}

impl Expression {
    pub fn eval(&self, v: f64, params: &Params) -> f64 {
        self.root.eval(v, params)
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Kind {
    Num(f64),
    Name(Vec<char>),
    Op(char),
    End,
}

#[derive(Debug, Clone, PartialEq)]
struct Token {
    kind: Kind,
    at: usize,
}

fn is_letter(c: char) -> bool {
    c.is_ascii_alphabetic() || ('α'..='ω').contains(&c) || ('Α'..='Ω').contains(&c)
}

fn tokenize(src: &[u16]) -> Result<Vec<Token>, Failure> {
    let digit = |u: Option<&u16>| u.is_some_and(|u| (0x30..=0x39).contains(u));
    let fail = |at| Failure {
        error: ExpressionError::Unexpected,
        at,
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < src.len() {
        let unit = src[i];
        let ch = char::from_u32(u32::from(unit)).unwrap_or('\u{fffd}');
        if unit == 0x20 || unit == 0x09 {
            i += 1;
        } else if digit(Some(&unit)) || (unit == 0x2e && digit(src.get(i + 1))) {
            let mut end = i;
            while end < src.len() && (digit(src.get(end)) || src[end] == 0x2e) {
                end += 1;
            }
            let text: String = src[i..end].iter().map(|u| char::from(*u as u8)).collect();
            let value = text.parse::<f64>().ok().filter(|v| v.is_finite());
            out.push(Token {
                kind: Kind::Num(value.ok_or_else(|| fail(i))?),
                at: i,
            });
            i = end;
        } else if "+-*/^()×·÷−,".contains(ch) {
            let op = match ch {
                '×' | '·' => '*',
                '÷' => '/',
                '−' => '-',
                c => c,
            };
            out.push(Token {
                kind: Kind::Op(op),
                at: i,
            });
            i += 1;
        } else if is_letter(ch) || ch == 'π' {
            let mut end = i;
            while end < src.len() {
                let c = char::from_u32(u32::from(src[end])).unwrap_or('\u{fffd}');
                if !(is_letter(c) || c == 'π') {
                    break;
                }
                end += 1;
            }
            let text = src[i..end]
                .iter()
                .map(|u| char::from_u32(u32::from(*u)).unwrap_or('\u{fffd}'))
                .collect();
            out.push(Token {
                kind: Kind::Name(text),
                at: i,
            });
            i = end;
        } else {
            return Err(fail(i));
        }
    }
    out.push(Token {
        kind: Kind::End,
        at: src.len(),
    });
    Ok(out)
}

struct Parser {
    tokens: Vec<Token>,
    i: usize,
    nodes: usize,
    depth: usize,
    variable: char,
    degrees: bool,
    params: Vec<char>,
}

type Parsed = Result<Node, Failure>;

impl Parser {
    fn peek(&self) -> &Token {
        &self.tokens[self.i]
    }

    fn fail(&self, error: ExpressionError, at: usize) -> Failure {
        Failure { error, at }
    }

    fn is_op(&self, op: char) -> bool {
        self.peek().kind == Kind::Op(op)
    }

    fn node(&mut self, n: Node) -> Parsed {
        self.nodes += 1;
        if self.nodes > MAX_NODES {
            return Err(self.fail(ExpressionError::TooComplex, self.peek().at));
        }
        Ok(n)
    }

    fn parse(&mut self) -> Parsed {
        let node = self.sum()?;
        let t = self.peek();
        if t.kind != Kind::End {
            let error = if t.kind == Kind::Op(')') {
                ExpressionError::Unclosed
            } else {
                ExpressionError::Unexpected
            };
            return Err(self.fail(error, t.at));
        }
        Ok(node)
    }

    fn sum(&mut self) -> Parsed {
        let mut left = self.product()?;
        loop {
            let plus = self.is_op('+');
            if !plus && !self.is_op('-') {
                return Ok(left);
            }
            self.i += 1;
            let right = Box::new(self.product()?);
            let a = Box::new(left);
            left = self.node(if plus {
                Node::Add(a, right)
            } else {
                Node::Sub(a, right)
            })?;
        }
    }

    fn product(&mut self) -> Parsed {
        let mut left = self.unary()?;
        loop {
            let times = self.is_op('*');
            if times || self.is_op('/') {
                self.i += 1;
                let right = Box::new(self.unary()?);
                let a = Box::new(left);
                left = self.node(if times {
                    Node::Mul(a, right)
                } else {
                    Node::Div(a, right)
                })?;
            } else if self.starts_factor() {
                // Implicit multiplication: 2x, 2(x+1), (x+1)(x-1), a sin x.
                let right = Box::new(self.unary()?);
                left = self.node(Node::Mul(Box::new(left), right))?;
            } else {
                return Ok(left);
            }
        }
    }

    fn starts_factor(&self) -> bool {
        matches!(
            self.peek().kind,
            Kind::Num(_) | Kind::Name(_) | Kind::Op('(')
        )
    }

    fn unary(&mut self) -> Parsed {
        if self.is_op('-') {
            self.i += 1;
            let a = self.unary()?;
            return self.node(Node::Neg(Box::new(a)));
        }
        if self.is_op('+') {
            self.i += 1;
            return self.unary();
        }
        self.power()
    }

    /// Right associative; the exponent may carry a sign (2^-x).
    fn power(&mut self) -> Parsed {
        let base = self.atom()?;
        if !self.is_op('^') {
            return Ok(base);
        }
        self.i += 1;
        let exponent = self.unary()?;
        self.node(Node::Pow(Box::new(base), Box::new(exponent)))
    }

    fn atom(&mut self) -> Parsed {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.fail(ExpressionError::TooComplex, self.peek().at));
        }
        let node = self.primary();
        self.depth -= 1;
        node
    }

    fn primary(&mut self) -> Parsed {
        let t = self.peek().clone();
        match t.kind {
            Kind::Num(value) => {
                self.i += 1;
                self.node(Node::Num(value))
            }
            Kind::Op('(') => {
                self.i += 1;
                let inner = self.sum()?;
                self.close()?;
                Ok(inner)
            }
            Kind::Name(text) => self.name(&text, t.at),
            Kind::End => Err(self.fail(ExpressionError::Unclosed, t.at)),
            Kind::Op(_) => Err(self.fail(ExpressionError::Unexpected, t.at)),
        }
    }

    fn close(&mut self) -> Result<(), Failure> {
        let t = self.peek();
        if t.kind != Kind::Op(')') {
            return Err(self.fail(ExpressionError::Unclosed, t.at));
        }
        self.i += 1;
        Ok(())
    }

    /// A run of letters: a function or constant when it starts with a known name (longest
    /// first), otherwise one letter, so `ax` reads as a*x and `xy`, `kx` need no spaces.
    fn name(&mut self, text: &[char], at: usize) -> Parsed {
        let starts = |name: &str| {
            let n = name.chars().count();
            text.len() >= n && name.chars().zip(text).all(|(a, b)| a == *b)
        };
        let found = FUNCTIONS
            .iter()
            .filter(|(name, _, _)| starts(name))
            .max_by_key(|(name, _, _)| name.len());
        if let Some(&(name, func, arity)) = found {
            // Split the token: what follows the function name is read again.
            self.split(text, name.len(), at);
            return self.call(func, arity, at);
        }
        for (name, width) in [("pi", 2), ("π", 1)] {
            if starts(name) {
                self.split(text, width, at);
                return self.node(Node::Num(PI));
            }
        }
        let first = text[0];
        self.split(text, 1, at);
        if first == self.variable {
            return self.node(Node::Variable);
        }
        if first == 'e' {
            return self.node(Node::Num(E));
        }
        if !self.params.contains(&first) {
            self.params.push(first);
        }
        self.node(Node::Param(first))
    }

    /// Consumes `width` characters of the name token at the cursor; the remainder stays a token.
    fn split(&mut self, text: &[char], width: usize, at: usize) {
        let rest = &text[width..];
        let replacement = (!rest.is_empty()).then(|| Token {
            kind: Kind::Name(rest.to_vec()),
            at: at + width,
        });
        self.tokens.splice(self.i..=self.i, replacement);
    }

    fn call(&mut self, func: Func, arity: usize, at: usize) -> Parsed {
        let mut args = Vec::new();
        if self.is_op('(') {
            self.i += 1;
            args.push(self.sum()?);
            while self.is_op(',') {
                self.i += 1;
                args.push(self.sum()?);
            }
            self.close()?;
        } else {
            // sin x, sin 2x... without brackets takes the next power: sin x^2 is sin(x²).
            args.push(self.power()?);
        }
        if args.len() != arity {
            return Err(self.fail(ExpressionError::Unexpected, at));
        }
        let mut args = args.into_iter();
        let a = Box::new(args.next().expect("one argument"));
        match args.next() {
            None => self.node(Node::Call1(func, self.degrees, a)),
            Some(b) => self.node(Node::Call2(func, a, Box::new(b))),
        }
    }
}

/// Reads a formula. `degrees` makes the trigonometric functions take and return degrees.
pub fn compile(source: &str, variable: Variable, degrees: bool) -> Result<Expression, Failure> {
    let src: Vec<u16> = source
        .trim_matches(super::is_js_space)
        .encode_utf16()
        .collect();
    if src.is_empty() {
        return Err(Failure {
            error: ExpressionError::Empty,
            at: 0,
        });
    }
    if src.len() > MAX_EXPRESSION_LENGTH {
        return Err(Failure {
            error: ExpressionError::TooLong,
            at: MAX_EXPRESSION_LENGTH,
        });
    }
    let mut parser = Parser {
        tokens: tokenize(&src)?,
        i: 0,
        nodes: 0,
        depth: 0,
        variable: variable.letter(),
        degrees,
        params: Vec::new(),
    };
    let root = parser.parse()?;
    Ok(Expression {
        params: parser.params,
        root,
    })
}
