//! Landform weight rules: a tiny expression language compiled to stack
//! bytecode (pipeline §8.1, App. C.3; M2 Shape design §2).
//!
//! Rules such as `"bump(uplift, 0.2, 0.5) * humid"` are parsed once and
//! compiled to a compact word stream. The Tier A bake interprets it once per
//! texel on the GPU (f32); [`evaluate`] is the f64 CPU reference over the same
//! words, so rule edits never recompile a shader.
//!
//! # Grammar
//!
//! ```text
//! expr    := term (('+' | '-') term)*
//! term    := unary (('*' | '/') unary)*
//! unary   := ('-' | '+') unary | primary
//! primary := number | field | function '(' expr (',' expr)* ')' | '(' expr ')'
//! number  := digits ['.' digits] [('e' | 'E') ['+' | '-'] digits]  (also '.5')
//! ```
//!
//! Fields: see [`field`] (aliases `humid` = `moisture`, `rock_hardness` =
//! `hardness`). Functions: `smoothstep(e0, e1, x)`, `bump(x, a, b)`,
//! `clamp(x, lo, hi)`, `min(a, b)`, `max(a, b)`, `mix(a, b, t)`, `pow(x, y)`,
//! `abs(x)`. Subexpressions without fields are folded to one constant.
//!
//! # Bytecode (format version [`BYTECODE_VERSION`])
//!
//! A rule is a `u32` word stream of instructions. Each instruction is one
//! word `opcode | operand << 8` ([`op`]), except [`op::CONST`], which is
//! followed by one word holding the IEEE-754 **f32** bit pattern of the
//! constant (both interpreters read constants as f32 values). Only
//! [`op::FIELD`] has an operand (a [`field`] id); every other operand byte is
//! zero. The machine is a stack of at most [`MAX_RULE_STACK`] values; a rule
//! has at most [`MAX_RULE_OPS`] instructions including the final
//! [`op::END`], which returns the single remaining value. Operands are popped
//! in reverse: for `f(a, b, c)` the stack holds `.. a b c` (c on top), as the
//! source order reads.
//!
//! | opcode | name | stack effect |
//! |---|---|---|
//! | 0x00 | END | `v` → return `v` |
//! | 0x01 | CONST | → `f32::from_bits(next word)` |
//! | 0x02 | FIELD | → `fields[operand]` |
//! | 0x03 | ADD | `a b` → `a + b` |
//! | 0x04 | SUB | `a b` → `a − b` |
//! | 0x05 | MUL | `a b` → `a · b` |
//! | 0x06 | DIV | `a b` → `b ≠ 0 ? a / b : 0` |
//! | 0x07 | NEG | `a` → `−a` |
//! | 0x08 | MIN | `a b` → `min(a, b)` |
//! | 0x09 | MAX | `a b` → `max(a, b)` |
//! | 0x0A | ABS | `a` → `abs(a)` |
//! | 0x0B | POW | `x y` → `x > 0 ? x^y : 0` |
//! | 0x0C | CLAMP | `x lo hi` → `min(max(x, lo), hi)` |
//! | 0x0D | MIX | `a b t` → `a + (b − a)·t` |
//! | 0x0E | SMOOTHSTEP | `e0 e1 x` → `e0 = e1 ? (x < e0 ? 0 : 1) : s(clamp((x − e0)/(e1 − e0), 0, 1))`, `s(t) = t²(3 − 2t)`; `e0 > e1` gives a falling edge |
//! | 0x0F | BUMP | `x a b` → `b > a ∧ abs(t) < 1 ? (1 − t²)² : 0`, `t = (2x − a − b)/(b − a)` |
//!
//! A *set* buffer ([`encode_set`]) concatenates the rules of one landform set
//! behind a header:
//!
//! | word | content |
//! |---|---|
//! | 0 | [`BYTECODE_VERSION`] |
//! | 1 | rule count `N` (≤ [`MAX_SET_RULES`]) |
//! | 2 | flags: bit 0 = normalise |
//! | 3 | fallback rule index |
//! | 4 .. 4 + N | word offset of rule `i` from the start of the buffer |
//! | 4 + N .. | rule code |
//!
//! Weights ([`evaluate_set`]): `w_i = clamp(rule_i, 0, 1)` (non-finite → 0,
//! the unorm8 storage range); `w_fallback += max(0, FALLBACK_FLOOR − Σw)` (the
//! sum never reaches zero and stays continuous); if normalising,
//! `w_i /= Σw`.
use super::LandformError;

/// Bytecode format version (header word 0).
pub const BYTECODE_VERSION: u32 = 1;
/// Instruction limit per rule, including the final END.
pub const MAX_RULE_OPS: usize = 64;
/// Stack depth limit of the interpreters.
pub const MAX_RULE_STACK: usize = 16;
/// Rule limit of one set buffer.
pub const MAX_SET_RULES: usize = 16;
/// Header words before the rule offsets.
pub const SET_HEADER_WORDS: usize = 4;
/// Weight sum below which the fallback landform takes the remainder.
pub const FALLBACK_FLOOR: f64 = 0.0625;

/// Opcodes (low 8 bits of an instruction word).
pub mod op {
    pub const END: u32 = 0x00;
    pub const CONST: u32 = 0x01;
    pub const FIELD: u32 = 0x02;
    pub const ADD: u32 = 0x03;
    pub const SUB: u32 = 0x04;
    pub const MUL: u32 = 0x05;
    pub const DIV: u32 = 0x06;
    pub const NEG: u32 = 0x07;
    pub const MIN: u32 = 0x08;
    pub const MAX: u32 = 0x09;
    pub const ABS: u32 = 0x0A;
    pub const POW: u32 = 0x0B;
    pub const CLAMP: u32 = 0x0C;
    pub const MIX: u32 = 0x0D;
    pub const SMOOTHSTEP: u32 = 0x0E;
    pub const BUMP: u32 = 0x0F;
}

/// Rule input field ids (FIELD operand; index into the fields array). The
/// Tier A host fills them per texel with these conventions.
pub mod field {
    /// Orogenic uplift, 0..1.
    pub const UPLIFT: u32 = 0;
    /// Deposited sediment, 0..1.
    pub const SEDIMENT: u32 = 1;
    /// Moisture, 0..1 (alias `humid`).
    pub const MOISTURE: u32 = 2;
    /// `1 − moisture`.
    pub const ARID: u32 = 3;
    /// Surface temperature, °C.
    pub const TEMPERATURE: u32 = 4;
    /// `1 − smoothstep(−15, 5, temperature)`.
    pub const COLD: u32 = 5;
    /// Rock hardness, 0..1 (alias `rock_hardness`).
    pub const HARDNESS: u32 = 6;
    /// Macro slope magnitude, rise over run.
    pub const SLOPE_MACRO: u32 = 7;
    /// Macro elevation above sea level, metres.
    pub const ELEVATION: u32 = 8;
    /// `|boundary_coord|`, metres.
    pub const BOUNDARY_DISTANCE: u32 = 9;
    /// Volcanic mask, 0..1.
    pub const VOLCANIC: u32 = 10;
    /// 1 below sea level, else 0.
    pub const OCEAN: u32 = 11;
    /// Number of fields.
    pub const COUNT: usize = 12;
}

/// Field names by id, then aliases.
const FIELD_NAMES: [(&str, u32); 14] = [
    ("uplift", field::UPLIFT),
    ("sediment", field::SEDIMENT),
    ("moisture", field::MOISTURE),
    ("arid", field::ARID),
    ("temperature", field::TEMPERATURE),
    ("cold", field::COLD),
    ("hardness", field::HARDNESS),
    ("slope_macro", field::SLOPE_MACRO),
    ("elevation", field::ELEVATION),
    ("boundary_distance", field::BOUNDARY_DISTANCE),
    ("volcanic", field::VOLCANIC),
    ("ocean", field::OCEAN),
    ("humid", field::MOISTURE),
    ("rock_hardness", field::HARDNESS),
];

/// Functions: name, opcode, arity.
const FUNCTIONS: [(&str, u32, usize); 8] = [
    ("smoothstep", op::SMOOTHSTEP, 3),
    ("bump", op::BUMP, 3),
    ("clamp", op::CLAMP, 3),
    ("min", op::MIN, 2),
    ("max", op::MAX, 2),
    ("mix", op::MIX, 3),
    ("pow", op::POW, 2),
    ("abs", op::ABS, 1),
];

/// Rule inputs indexed by [`field`] id.
pub type RuleFields = [f64; field::COUNT];

/// A compiled weight rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    words: Vec<u32>,
}

impl Rule {
    /// Parse and compile `source`.
    pub fn compile(source: &str) -> Result<Self, LandformError> {
        let tokens = lex(source)?;
        let mut parser = Parser {
            tokens: &tokens,
            at: 0,
        };
        let ast = parser.expr()?;
        if parser.at != tokens.len() {
            return Err(parser.error("unexpected trailing input"));
        }
        let mut words = Vec::new();
        emit(&fold(ast), &mut words);
        words.push(op::END);
        validate(&words)?;
        Ok(Self { words })
    }

    /// The rule's words (without a set header).
    pub fn words(&self) -> &[u32] {
        &self.words
    }

    /// f64 evaluation of the rule.
    pub fn evaluate(&self, fields: &RuleFields) -> f64 {
        // Compiled rules are validated, so evaluation cannot fail.
        evaluate(&self.words, fields).unwrap_or(0.0)
    }
}

/// Reference f64 interpreter of one rule's words (the GPU mirrors it in f32).
/// Fails on malformed code: unknown opcode, stack under/overflow, a bad field
/// id, a missing END or more than [`MAX_RULE_OPS`] instructions.
pub fn evaluate(code: &[u32], fields: &RuleFields) -> Result<f64, LandformError> {
    let bad = |why: &str| LandformError::Bytecode(why.to_string());
    let mut stack = [0.0f64; MAX_RULE_STACK];
    let mut top = 0usize;
    let mut at = 0usize;
    for _ in 0..MAX_RULE_OPS {
        let word = *code.get(at).ok_or_else(|| bad("missing END"))?;
        at += 1;
        let (opcode, operand) = (word & 0xff, word >> 8);
        let (pops, pushes) = stack_effect(opcode).ok_or_else(|| bad("unknown opcode"))?;
        if opcode != op::FIELD && operand != 0 {
            return Err(bad("operand on an opcode without one"));
        }
        if top < pops || top - pops + pushes > MAX_RULE_STACK {
            return Err(bad("stack underflow or overflow"));
        }
        let args = &stack[top - pops..top];
        let value = match opcode {
            op::END => {
                return if top == 1 {
                    Ok(stack[0])
                } else {
                    Err(bad("END with more than one value"))
                };
            }
            op::CONST => {
                let bits = *code.get(at).ok_or_else(|| bad("CONST without a value"))?;
                at += 1;
                f64::from(f32::from_bits(bits))
            }
            op::FIELD => *fields
                .get(operand as usize)
                .ok_or_else(|| bad("unknown field"))?,
            _ => apply(opcode, args),
        };
        top -= pops;
        stack[top] = value;
        top += 1;
    }
    Err(bad("more than MAX_RULE_OPS instructions"))
}

/// `(pops, pushes)` of an opcode.
fn stack_effect(opcode: u32) -> Option<(usize, usize)> {
    Some(match opcode {
        op::END => (1, 0),
        op::CONST | op::FIELD => (0, 1),
        op::NEG | op::ABS => (1, 1),
        op::ADD | op::SUB | op::MUL | op::DIV | op::MIN | op::MAX | op::POW => (2, 1),
        op::CLAMP | op::MIX | op::SMOOTHSTEP | op::BUMP => (3, 1),
        _ => return None,
    })
}

/// Arithmetic of every value opcode; `args` in source order.
fn apply(opcode: u32, args: &[f64]) -> f64 {
    match opcode {
        op::ADD => args[0] + args[1],
        op::SUB => args[0] - args[1],
        op::MUL => args[0] * args[1],
        op::DIV => {
            if args[1] != 0.0 {
                args[0] / args[1]
            } else {
                0.0
            }
        }
        op::NEG => -args[0],
        op::MIN => args[0].min(args[1]),
        op::MAX => args[0].max(args[1]),
        op::ABS => args[0].abs(),
        op::POW => {
            if args[0] > 0.0 {
                args[0].powf(args[1])
            } else {
                0.0
            }
        }
        op::CLAMP => args[0].max(args[1]).min(args[2]),
        op::MIX => args[0] + (args[1] - args[0]) * args[2],
        op::SMOOTHSTEP => smoothstep(args[0], args[1], args[2]),
        op::BUMP => bump(args[0], args[1], args[2]),
        _ => 0.0,
    }
}

/// SMOOTHSTEP semantics (also used by the host's `cold` field).
pub fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    if e0 == e1 {
        return if x < e0 { 0.0 } else { 1.0 };
    }
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// BUMP semantics: a smooth hump on `[a, b]`, 1 at the centre.
pub fn bump(x: f64, a: f64, b: f64) -> f64 {
    if b <= a {
        return 0.0;
    }
    let t = (2.0 * x - a - b) / (b - a);
    if t.abs() < 1.0 {
        let s = 1.0 - t * t;
        s * s
    } else {
        0.0
    }
}

/// Static checks of compiled words: instruction count, stack depth, field
/// ids, a single final END.
fn validate(code: &[u32]) -> Result<(), LandformError> {
    let mut at = 0;
    let mut depth = 0usize;
    let mut ops = 0;
    while at < code.len() {
        let (opcode, operand) = (code[at] & 0xff, code[at] >> 8);
        let (pops, pushes) = stack_effect(opcode)
            .ok_or_else(|| LandformError::Bytecode(format!("unknown opcode {opcode:#x}")))?;
        ops += 1;
        if opcode == op::FIELD && operand as usize >= field::COUNT {
            return Err(LandformError::Bytecode("unknown field".into()));
        }
        depth = depth + pushes - pops.min(depth);
        if depth > MAX_RULE_STACK {
            return Err(LandformError::Rule(format!(
                "needs a stack deeper than {MAX_RULE_STACK}"
            )));
        }
        at += if opcode == op::CONST { 2 } else { 1 };
        if opcode == op::END {
            break;
        }
    }
    if ops > MAX_RULE_OPS {
        return Err(LandformError::Rule(format!(
            "{ops} instructions exceed the limit of {MAX_RULE_OPS}"
        )));
    }
    if at != code.len() || code.last() != Some(&op::END) {
        return Err(LandformError::Bytecode("END must end the rule".into()));
    }
    Ok(())
}

/// Reference set encoding (layout in the module docs).
pub fn encode_set(
    rules: &[Rule],
    normalize: bool,
    fallback: usize,
) -> Result<Vec<u32>, LandformError> {
    if rules.is_empty() || rules.len() > MAX_SET_RULES || fallback >= rules.len() {
        return Err(LandformError::Set(format!(
            "a set needs 1..={MAX_SET_RULES} rules and a valid fallback"
        )));
    }
    let mut words = vec![
        BYTECODE_VERSION,
        rules.len() as u32,
        u32::from(normalize),
        fallback as u32,
    ];
    let mut offset = SET_HEADER_WORDS + rules.len();
    for rule in rules {
        words.push(offset as u32);
        offset += rule.words.len();
    }
    for rule in rules {
        words.extend_from_slice(&rule.words);
    }
    Ok(words)
}

/// Reference evaluation of a whole set buffer: the landform weights of one
/// texel (module docs).
pub fn evaluate_set(words: &[u32], fields: &RuleFields) -> Result<Vec<f64>, LandformError> {
    let bad = |why: &str| LandformError::Bytecode(why.to_string());
    if words.len() < SET_HEADER_WORDS || words[0] != BYTECODE_VERSION {
        return Err(bad("not a version 1 set buffer"));
    }
    let count = words[1] as usize;
    let normalize = words[2] & 1 == 1;
    let fallback = words[3] as usize;
    if count == 0 || count > MAX_SET_RULES || fallback >= count {
        return Err(bad("bad set header"));
    }
    let mut weights = Vec::with_capacity(count);
    for i in 0..count {
        let offset = *words
            .get(SET_HEADER_WORDS + i)
            .ok_or_else(|| bad("missing rule offset"))? as usize;
        let code = words.get(offset..).ok_or_else(|| bad("bad rule offset"))?;
        let w = evaluate(code, fields)?;
        weights.push(if w.is_finite() {
            w.clamp(0.0, 1.0)
        } else {
            0.0
        });
    }
    let sum: f64 = weights.iter().sum();
    weights[fallback] += (FALLBACK_FLOOR - sum).max(0.0);
    if normalize {
        let sum: f64 = weights.iter().sum();
        weights.iter_mut().for_each(|w| *w /= sum);
    }
    Ok(weights)
}

/// One published test vector for the GPU interpreter: source, expected
/// words, inputs and expected f64 result.
#[derive(Debug, Clone, Copy)]
pub struct RuleTestVector {
    pub source: &'static str,
    pub words: &'static [u32],
    pub fields: RuleFields,
    pub expected: f64,
}

const fn fields_with(uplift: f64, sediment: f64, moisture: f64, hardness: f64) -> RuleFields {
    let mut f = [0.0; field::COUNT];
    f[field::UPLIFT as usize] = uplift;
    f[field::SEDIMENT as usize] = sediment;
    f[field::MOISTURE as usize] = moisture;
    f[field::ARID as usize] = 1.0 - moisture;
    f[field::HARDNESS as usize] = hardness;
    f
}

/// Test vectors (expected results computed by hand; constants are the f32
/// values of the literals).
pub const RULE_TEST_VECTORS: &[RuleTestVector] = &[
    RuleTestVector {
        source: "uplift",
        words: &[0x0000_0002, op::END],
        fields: fields_with(0.25, 0.0, 0.0, 0.0),
        expected: 0.25,
    },
    RuleTestVector {
        // Folded: 1 − 0.25 = 0.75.
        source: "1 - 0.25",
        words: &[op::CONST, 0x3f40_0000, op::END],
        fields: fields_with(0.0, 0.0, 0.0, 0.0),
        expected: 0.75,
    },
    RuleTestVector {
        // t = (0.625 − 0.5)/0.25 = 0.5 → 0.5.
        source: "smoothstep(0.5, 0.75, uplift)",
        words: &[
            op::CONST,
            0x3f00_0000,
            op::CONST,
            0x3f40_0000,
            0x0000_0002,
            op::SMOOTHSTEP,
            op::END,
        ],
        fields: fields_with(0.625, 0.0, 0.0, 0.0),
        expected: 0.5,
    },
    RuleTestVector {
        // t = (0.375·2 − 0.75)/0.5 = 0 → bump 1; × humid 0.5.
        source: "bump(uplift, 0.25, 0.5) * humid",
        words: &[
            0x0000_0002,
            op::CONST,
            0x3e80_0000,
            op::CONST,
            0x3f00_0000,
            op::BUMP,
            0x0000_0202,
            op::MUL,
            op::END,
        ],
        fields: fields_with(0.375, 0.0, 0.5, 0.0),
        expected: 0.5,
    },
    RuleTestVector {
        // (1 − 0.5) · smoothstep(0.25, 0.75, 0.625): t = 0.75 → 0.84375; · 0.5.
        source: "(1 - uplift) * smoothstep(0.25, 0.75, sediment)",
        words: &[
            op::CONST,
            0x3f80_0000,
            0x0000_0002,
            op::SUB,
            op::CONST,
            0x3e80_0000,
            op::CONST,
            0x3f40_0000,
            0x0000_0102,
            op::SMOOTHSTEP,
            op::MUL,
            op::END,
        ],
        fields: fields_with(0.5, 0.625, 0.0, 0.0),
        expected: 0.421875,
    },
    RuleTestVector {
        // arid² · (1 − rock_hardness) / 2 = 0.5625 · 0.5 / 2.
        source: "arid * arid * (1 - rock_hardness) / 2",
        words: &[
            0x0000_0302,
            0x0000_0302,
            op::MUL,
            op::CONST,
            0x3f80_0000,
            0x0000_0602,
            op::SUB,
            op::MUL,
            op::CONST,
            0x4000_0000,
            op::DIV,
            op::END,
        ],
        fields: fields_with(0.0, 0.0, 0.25, 0.5),
        expected: 0.140625,
    },
    RuleTestVector {
        // clamp(−0.5, 0, 1) = 0; max(0, mix(0.5, 1, 0.5) = 0.75); pow(0.75, 2).
        source: "pow(max(clamp(-uplift, 0, 1), mix(0.5, 1, sediment)), 2)",
        words: &[
            0x0000_0002,
            op::NEG,
            op::CONST,
            0x0000_0000,
            op::CONST,
            0x3f80_0000,
            op::CLAMP,
            op::CONST,
            0x3f00_0000,
            op::CONST,
            0x3f80_0000,
            0x0000_0102,
            op::MIX,
            op::MAX,
            op::CONST,
            0x4000_0000,
            op::POW,
            op::END,
        ],
        fields: fields_with(0.5, 0.5, 0.0, 0.0),
        expected: 0.5625,
    },
    RuleTestVector {
        // Division by zero yields 0; abs of a negative field.
        source: "uplift / sediment + abs(min(-moisture, 0))",
        words: &[
            0x0000_0002,
            0x0000_0102,
            op::DIV,
            0x0000_0202,
            op::NEG,
            op::CONST,
            0x0000_0000,
            op::MIN,
            op::ABS,
            op::ADD,
            op::END,
        ],
        fields: fields_with(0.5, 0.0, 0.25, 0.0),
        expected: 0.25,
    },
];

// ---------------------------------------------------------------- parsing

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(f64),
    Ident(String),
    Symbol(char),
}

fn lex(source: &str) -> Result<Vec<(Token, usize)>, LandformError> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let c = bytes[at] as char;
        if c.is_ascii_whitespace() {
            at += 1;
        } else if c.is_ascii_digit() || c == '.' {
            let start = at;
            while at < bytes.len() && (bytes[at].is_ascii_digit() || bytes[at] == b'.') {
                at += 1;
            }
            if at < bytes.len() && (bytes[at] == b'e' || bytes[at] == b'E') {
                at += 1;
                if at < bytes.len() && (bytes[at] == b'+' || bytes[at] == b'-') {
                    at += 1;
                }
                while at < bytes.len() && bytes[at].is_ascii_digit() {
                    at += 1;
                }
            }
            let text = &source[start..at];
            let value: f64 = text
                .parse()
                .map_err(|_| LandformError::Rule(format!("bad number '{text}' at {start}")))?;
            tokens.push((Token::Number(value), start));
        } else if c.is_ascii_alphabetic() || c == '_' {
            let start = at;
            while at < bytes.len() && (bytes[at].is_ascii_alphanumeric() || bytes[at] == b'_') {
                at += 1;
            }
            tokens.push((Token::Ident(source[start..at].to_string()), start));
        } else if "+-*/(),".contains(c) {
            tokens.push((Token::Symbol(c), at));
            at += 1;
        } else {
            return Err(LandformError::Rule(format!("unexpected '{c}' at {at}")));
        }
    }
    Ok(tokens)
}

#[derive(Debug, Clone, PartialEq)]
enum Ast {
    Const(f64),
    Field(u32),
    Op(u32, Vec<Ast>),
}

struct Parser<'a> {
    tokens: &'a [(Token, usize)],
    at: usize,
}

impl Parser<'_> {
    fn error(&self, what: &str) -> LandformError {
        let position = self.tokens.get(self.at).map_or_else(
            || "end of input".to_string(),
            |(_, at)| format!("position {at}"),
        );
        LandformError::Rule(format!("{what} at {position}"))
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at).map(|(t, _)| t)
    }

    fn eat(&mut self, symbol: char) -> bool {
        if self.peek() == Some(&Token::Symbol(symbol)) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn expr(&mut self) -> Result<Ast, LandformError> {
        let mut left = self.term()?;
        loop {
            let opcode = if self.eat('+') {
                op::ADD
            } else if self.eat('-') {
                op::SUB
            } else {
                return Ok(left);
            };
            left = Ast::Op(opcode, vec![left, self.term()?]);
        }
    }

    fn term(&mut self) -> Result<Ast, LandformError> {
        let mut left = self.unary()?;
        loop {
            let opcode = if self.eat('*') {
                op::MUL
            } else if self.eat('/') {
                op::DIV
            } else {
                return Ok(left);
            };
            left = Ast::Op(opcode, vec![left, self.unary()?]);
        }
    }

    fn unary(&mut self) -> Result<Ast, LandformError> {
        if self.eat('-') {
            Ok(Ast::Op(op::NEG, vec![self.unary()?]))
        } else if self.eat('+') {
            self.unary()
        } else {
            self.primary()
        }
    }

    fn primary(&mut self) -> Result<Ast, LandformError> {
        match self.peek().cloned() {
            Some(Token::Number(value)) => {
                self.at += 1;
                Ok(Ast::Const(value))
            }
            Some(Token::Symbol('(')) => {
                self.at += 1;
                let inner = self.expr()?;
                if !self.eat(')') {
                    return Err(self.error("expected ')'"));
                }
                Ok(inner)
            }
            Some(Token::Ident(name)) => {
                self.at += 1;
                if let Some(&(_, opcode, arity)) = FUNCTIONS.iter().find(|f| f.0 == name) {
                    if !self.eat('(') {
                        return Err(self.error(&format!("expected '(' after {name}")));
                    }
                    let mut args = vec![self.expr()?];
                    while self.eat(',') {
                        args.push(self.expr()?);
                    }
                    if !self.eat(')') {
                        return Err(self.error("expected ')'"));
                    }
                    if args.len() != arity {
                        return Err(LandformError::Rule(format!(
                            "{name} takes {arity} arguments, got {}",
                            args.len()
                        )));
                    }
                    Ok(Ast::Op(opcode, args))
                } else if let Some(&(_, id)) = FIELD_NAMES.iter().find(|f| f.0 == name) {
                    Ok(Ast::Field(id))
                } else {
                    self.at -= 1;
                    Err(self.error(&format!("unknown field or function '{name}'")))
                }
            }
            _ => Err(self.error("expected a number, field, function or '('")),
        }
    }
}

/// Fold field-free subtrees into one constant (evaluated in f64 with the f32
/// values of the literals, then stored as f32).
fn fold(ast: Ast) -> Ast {
    match ast {
        Ast::Op(opcode, args) => {
            let args: Vec<Ast> = args.into_iter().map(fold).collect();
            let constants: Option<Vec<f64>> = args
                .iter()
                .map(|a| match a {
                    Ast::Const(v) => Some(f64::from(*v as f32)),
                    _ => None,
                })
                .collect();
            match constants {
                Some(values) => Ast::Const(apply(opcode, &values)),
                None => Ast::Op(opcode, args),
            }
        }
        other => other,
    }
}

fn emit(ast: &Ast, words: &mut Vec<u32>) {
    match ast {
        Ast::Const(v) => {
            words.push(op::CONST);
            words.push((*v as f32).to_bits());
        }
        Ast::Field(id) => words.push(op::FIELD | id << 8),
        Ast::Op(opcode, args) => {
            for arg in args {
                emit(arg, words);
            }
            words.push(*opcode);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn published_test_vectors_compile_and_evaluate() {
        for v in RULE_TEST_VECTORS {
            let rule = Rule::compile(v.source).unwrap();
            assert_eq!(rule.words(), v.words, "{}", v.source);
            let got = evaluate(v.words, &v.fields).unwrap();
            assert!((got - v.expected).abs() < 1e-7, "{}: {got}", v.source);
        }
    }

    #[test]
    fn operators_follow_precedence_and_associativity() {
        let f = fields_with(0.5, 0.25, 0.0, 0.0);
        let eval = |s: &str| Rule::compile(s).unwrap().evaluate(&f);
        assert_eq!(eval("1 - uplift - sediment"), 0.25);
        assert_eq!(eval("uplift + sediment * 2"), 1.0);
        assert_eq!(eval("(uplift + sediment) * 2"), 1.5);
        assert_eq!(eval("uplift / sediment / 2"), 1.0);
        assert_eq!(eval("--uplift"), 0.5);
        assert_eq!(eval("-uplift * 2"), -1.0);
        assert!((eval("1e-1 * 10 + .5") - 1.5).abs() < 1e-6);
        assert_eq!(
            eval("smoothstep(5, -10, temperature)"),
            smoothstep(5.0, -10.0, 0.0)
        );
        assert!(smoothstep(5.0, -10.0, 0.0) > 0.25);
        assert_eq!(eval("smoothstep(0.5, 0.5, uplift)"), 1.0);
        assert_eq!(eval("bump(uplift, 0.5, 0.2)"), 0.0);
        assert_eq!(eval("pow(-uplift, 2)"), 0.0);
    }

    #[test]
    fn errors_are_reported() {
        for bad in [
            "",
            "uplift +",
            "upflit",
            "smoothstep(1, 2)",
            "min(1, 2, 3)",
            "(uplift",
            "uplift)",
            "uplift $ 2",
            "abs uplift",
            "1..2",
        ] {
            assert!(Rule::compile(bad).is_err(), "{bad}");
        }
        // 33 fields joined by '+' are 66 instructions.
        let long = vec!["uplift"; 33].join(" + ");
        assert!(Rule::compile(&long).is_err());
        let fine = vec!["uplift"; 32].join(" + ");
        assert_eq!(Rule::compile(&fine).unwrap().words().len(), 64);
        // Right-nested additions need a deep stack.
        let deep = format!("{}uplift{}", "uplift + (".repeat(16), ")".repeat(16));
        assert!(Rule::compile(&deep).is_err());
    }

    #[test]
    fn interpreter_rejects_malformed_code() {
        let f = [0.0; field::COUNT];
        for bad in [
            vec![],
            vec![op::ADD, op::END],
            vec![op::FIELD | 12 << 8, op::END],
            vec![op::CONST],
            vec![0x7f, op::END],
            vec![op::FIELD, op::FIELD, op::END],
            vec![op::ADD | 1 << 8, op::END],
        ] {
            assert!(evaluate(&bad, &f).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn set_encoding_round_trips_and_weights_normalise() {
        let rules: Vec<Rule> = [
            "smoothstep(0.5, 0.8, uplift)",
            "bump(uplift, 0.2, 0.5)",
            "0",
        ]
        .iter()
        .map(|s| Rule::compile(s).unwrap())
        .collect();
        let words = encode_set(&rules, true, 2).unwrap();
        assert_eq!(&words[..4], &[BYTECODE_VERSION, 3, 1, 2]);
        for (i, rule) in rules.iter().enumerate() {
            let offset = words[SET_HEADER_WORDS + i] as usize;
            assert_eq!(&words[offset..offset + rule.words().len()], rule.words());
        }
        for uplift in [0.0, 0.1, 0.35, 0.6, 0.9, 1.0] {
            let w = evaluate_set(&words, &fields_with(uplift, 0.0, 0.0, 0.0)).unwrap();
            assert!((w.iter().sum::<f64>() - 1.0).abs() < 1e-12);
            assert!(w.iter().all(|w| (0.0..=1.0).contains(w)));
            if uplift == 0.0 {
                assert_eq!(w, vec![0.0, 0.0, 1.0]);
            }
        }
        // Continuity of the fallback across the floor.
        let a = evaluate_set(&words, &fields_with(0.2 + 1e-9, 0.0, 0.0, 0.0)).unwrap();
        let b = evaluate_set(&words, &fields_with(0.2, 0.0, 0.0, 0.0)).unwrap();
        assert!((a[2] - b[2]).abs() < 1e-6);
        let raw = encode_set(&rules, false, 2).unwrap();
        let w = evaluate_set(&raw, &fields_with(0.9, 0.0, 0.0, 0.0)).unwrap();
        assert_eq!(w, vec![1.0, 0.0, 0.0]);
        assert!(encode_set(&rules, true, 3).is_err());
        assert!(evaluate_set(&words[..3], &[0.0; field::COUNT]).is_err());
    }
}
