//! A deliberately minimal C AST — `docs/rust-port-design.md` §7's `simpleC` model, as plain Rust
//! enums/structs (no Xcore/EMF needed, per that section: "this was always 'just a small C AST'").
//!
//! Scoped to exactly what [`crate::writec`] needs to build: function bodies with the handful of
//! statement/expression shapes the demand-driven generator emits (decl/assign/if-else/for/return),
//! plus a `Raw(String)` expression escape hatch. Preprocessor lines (`#include`, `#define` macros,
//! global variable declarations) are plain pre-formatted text rather than their own AST nodes —
//! they're never rewritten or pattern-matched on after being produced, only ever concatenated
//! verbatim into the output, so a structured representation would buy nothing here (this crate's
//! actual "affine/conditional expression" converters lean entirely on isl's own C-format printer —
//! see `docs/rust-port-design.md` §5/§7 — rather than reimplementing one).

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CType {
    Float,
    Long,
    Char,
    Void,
    /// `n` levels of `*` on top of the base type (e.g. `Ptr(Float, 2)` = `float**`).
    Ptr(Box<CType>, u32),
}

impl CType {
    pub fn ptr(base: CType, levels: u32) -> CType {
        if levels == 0 {
            base
        } else {
            CType::Ptr(Box::new(base), levels)
        }
    }
}

impl fmt::Display for CType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CType::Float => write!(f, "float"),
            CType::Long => write!(f, "long"),
            CType::Char => write!(f, "char"),
            CType::Void => write!(f, "void"),
            CType::Ptr(base, levels) => {
                write!(f, "{base}")?;
                for _ in 0..*levels {
                    write!(f, "*")?;
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum Expr {
    /// Pre-rendered C text, embedded verbatim — the escape hatch every affine/boolean condition
    /// goes through, since isl's own C-format printer (`Format::C`) already produces valid C for
    /// those (see the module doc).
    Raw(String),
    Call(String, Vec<Expr>),
    /// `cond ? then : else`.
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expr::Raw(s) => write!(f, "{s}"),
            Expr::Call(name, args) => {
                write!(f, "{name}(")?;
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{a}")?;
                }
                write!(f, ")")
            }
            Expr::Ternary(c, t, e) => write!(f, "({c}) ? ({t}) : ({e})"),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Decl {
        ty: CType,
        name: String,
        init: Option<Expr>,
    },
    Assign {
        target: Expr,
        value: Expr,
    },
    If {
        cond: Expr,
        then_branch: Vec<Stmt>,
        else_branch: Vec<Stmt>,
    },
    /// A `for` loop whose header pieces are pre-rendered C text (isl's `AstNode::for_init`/
    /// `for_cond`/`for_inc` C-format text — see [`crate::convert`]).
    For {
        iterator: String,
        init: String,
        cond: String,
        inc: String,
        body: Vec<Stmt>,
    },
    Return(Option<Expr>),
    Expr(Expr),
    /// A single already-formatted line, emitted verbatim (blank lines, comments, `#define`/`#undef`
    /// pairs that bracket a loop nest with reduce-loop macros — see [`crate::writec`]).
    Raw(String),
}

pub struct Function {
    pub return_type: CType,
    pub name: String,
    pub params: Vec<(CType, String)>,
    pub body: Vec<Stmt>,
    /// Emitted `static`, matching every generated helper (`eval_<Var>`, `reduce<N>`) except the
    /// system's own public entry point.
    pub is_static: bool,
}

impl Function {
    pub fn signature(&self) -> String {
        let params = if self.params.is_empty() {
            "void".to_string()
        } else {
            self.params
                .iter()
                .map(|(ty, name)| format!("{ty} {name}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        format!(
            "{}{} {}({})",
            if self.is_static { "static " } else { "" },
            self.return_type,
            self.name,
            params
        )
    }
}

const INDENT: &str = "\t";

fn write_stmts(out: &mut String, stmts: &[Stmt], depth: usize) {
    for s in stmts {
        write_stmt(out, s, depth);
    }
}

fn write_stmt(out: &mut String, stmt: &Stmt, depth: usize) {
    let pad = INDENT.repeat(depth);
    match stmt {
        Stmt::Decl { ty, name, init } => {
            if let Some(init) = init {
                out.push_str(&format!("{pad}{ty} {name} = {init};\n"));
            } else {
                out.push_str(&format!("{pad}{ty} {name};\n"));
            }
        }
        Stmt::Assign { target, value } => {
            out.push_str(&format!("{pad}{target} = {value};\n"));
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
        } => {
            out.push_str(&format!("{pad}if ({cond}) {{\n"));
            write_stmts(out, then_branch, depth + 1);
            if else_branch.is_empty() {
                out.push_str(&format!("{pad}}}\n"));
            } else {
                out.push_str(&format!("{pad}}}\n{pad}else {{\n"));
                write_stmts(out, else_branch, depth + 1);
                out.push_str(&format!("{pad}}}\n"));
            }
        }
        Stmt::For {
            iterator,
            init,
            cond,
            inc,
            body,
        } => {
            out.push_str(&format!(
                "{pad}for ({iterator} = {init}; {cond}; {iterator} += {inc}) {{\n"
            ));
            write_stmts(out, body, depth + 1);
            out.push_str(&format!("{pad}}}\n"));
        }
        Stmt::Return(None) => out.push_str(&format!("{pad}return;\n")),
        Stmt::Return(Some(e)) => out.push_str(&format!("{pad}return {e};\n")),
        Stmt::Expr(e) => out.push_str(&format!("{pad}{e};\n")),
        Stmt::Raw(s) => {
            if s.is_empty() {
                out.push('\n');
            } else {
                out.push_str(&format!("{pad}{s}\n"));
            }
        }
    }
}

impl fmt::Display for Function {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{} {{", self.signature())?;
        let mut body = String::new();
        write_stmts(&mut body, &self.body, 1);
        f.write_str(&body)?;
        writeln!(f, "}}")
    }
}
