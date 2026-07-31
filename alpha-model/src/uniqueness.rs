//! Phase 5 of the six-phase pipeline (`docs/rust-port-design.md` §6): name-uniqueness checks
//! (`AlphaNameUniquenessChecker` in the source Java). Two scopes:
//! - Program-wide ([`check_program_uniqueness`]): duplicate systems/external functions, keyed by
//!   fully-qualified (package-prefixed) name, across every `Root` handed in together — the source
//!   system's `check(List<AlphaRoot>)`.
//! - Per-system ([`check_system_uniqueness`], also folded into [`check_program_uniqueness`] for
//!   every system it finds, matching the source's own composition): duplicate variable/`define`d
//!   object names, duplicate equation targets within a `SystemBody`, duplicate `constant` names.
//!
//! Unlike every other phase so far, these checks don't fail fast on the first problem — a
//! program can have several unrelated duplicate names, and the source system reports all of them
//! in one pass — so these functions return `Vec<Diagnostic>` directly (never `Result`), mirroring
//! `AlphaNameUniquenessChecker.check`'s `List<AlphaIssue>` return type.
//!
//! **Deliberate divergence from [`crate::resolve`]'s constant lookup**: a `constant`'s value
//! resolution (`resolve.rs`'s private `constants_in_scope`) walks *every* ancestor
//! `AlphaPackage`/`Root`, not just the system's immediate container — harmless for the whole
//! fixture corpus (no fixture nests packages deeply enough for it to matter) but, per the source
//! system's actual `AlphaUtil.getAlphaConstants` (only the system's *direct* container), broader
//! than intended and a latent shadowing hazard if a real program ever did nest packages with a
//! same-named constant at two levels. This module's duplicate-constant check deliberately uses
//! the narrower, source-faithful "direct container only" scope instead of reusing that helper —
//! flagged here rather than silently fixed, since `resolve.rs`'s behavior is out of this phase's
//! scope and already fixture-validated as-is.

use crate::diagnostic::Diagnostic;
use crate::walk::walk_expr;
use alpha_syntax::ast::{self, AstNode, Equation, Expr};
use alpha_syntax::syntax_kind::{SyntaxNode, SyntaxToken};
use std::collections::HashMap;

fn range_of(node: &SyntaxNode) -> (u32, u32) {
    let r = node.text_range();
    (r.start().into(), r.end().into())
}

fn token_range(t: &SyntaxToken) -> (u32, u32) {
    let r = t.text_range();
    (r.start().into(), r.end().into())
}

/// A variable (plain or fuzzy) or a `define`d object — the source system's
/// `AlphaNameUniquenessChecker.check(AlphaSystem)` puts both in one namespace since `Variable`
/// and `PolyhedralObject` are siblings under `AlphaSystem` that could otherwise collide.
enum NamedItem {
    Variable(ast::Variable),
    FuzzyVariable(ast::FuzzyVariable),
    Object(ast::PolyhedralObject),
}

impl NamedItem {
    fn name_token(&self) -> Option<SyntaxToken> {
        match self {
            NamedItem::Variable(v) => v.name(),
            NamedItem::FuzzyVariable(v) => v.name(),
            NamedItem::Object(o) => o.name(),
        }
    }

    fn syntax(&self) -> &SyntaxNode {
        match self {
            NamedItem::Variable(v) => v.syntax(),
            NamedItem::FuzzyVariable(v) => v.syntax(),
            NamedItem::Object(o) => o.syntax(),
        }
    }

    fn range(&self) -> (u32, u32) {
        self.name_token()
            .map(|t| token_range(&t))
            .unwrap_or_else(|| range_of(self.syntax()))
    }

    fn diagnostic(&self, name: &str) -> Diagnostic {
        let (start, end) = self.range();
        match self {
            // `FuzzyVariable extends Variable` in the source system, so both share this
            // diagnostic there too — see the module doc.
            NamedItem::Variable(_) | NamedItem::FuzzyVariable(_) => Diagnostic::DuplicateVariable {
                name: name.to_string(),
                start,
                end,
            },
            NamedItem::Object(_) => Diagnostic::DuplicatePolyhedralObject {
                name: name.to_string(),
                start,
                end,
            },
        }
    }
}

/// Every `Expr::Variable` reference anywhere within `expr`'s subtree — mirrors the source
/// system's `EcoreUtil.getAllContents(ue.getOutputExprs())` scan for `VariableExpression`, used
/// to find which variables a `UseEquation`'s output side writes to (its outputs can be an
/// arbitrary expression, not just a bare variable — e.g. wrapped in a `RestrictExpression`).
fn collect_variable_names(expr: &Expr, out: &mut Vec<String>) {
    walk_expr(expr, &mut |e| {
        if let Expr::Variable(v) = e {
            if let Some(name) = v.name() {
                out.push(name.text().to_string());
            }
        }
    });
}

/// Every `constant NAME = INT` declaration in the system's *direct* container only (its immediate
/// `AlphaPackage` or `Root`) — see the module doc for why this is narrower than
/// [`crate::resolve`]'s value-resolution lookup.
fn direct_container_constants(system: &ast::System) -> Vec<ast::AlphaConstant> {
    match system.syntax().parent() {
        Some(parent) => parent
            .children()
            .filter_map(ast::AlphaConstant::cast)
            .collect(),
        None => Vec::new(),
    }
}

/// Per-system name-uniqueness checks (§6, phase 5): duplicate variable/`define`d-object names,
/// duplicate equation targets within each `SystemBody`, duplicate `constant` declarations.
pub fn check_system_uniqueness(system: &ast::System) -> Vec<Diagnostic> {
    let mut diags = Vec::new();

    // 1. Variables (plain + fuzzy) and `define`d objects share one namespace.
    let mut items: HashMap<String, Vec<NamedItem>> = HashMap::new();
    let mut add_variables = |vars: Vec<ast::Variable>, fuzzy: Vec<ast::FuzzyVariable>| {
        for v in vars {
            if let Some(name) = v.name() {
                items
                    .entry(name.text().to_string())
                    .or_default()
                    .push(NamedItem::Variable(v));
            }
        }
        for v in fuzzy {
            if let Some(name) = v.name() {
                items
                    .entry(name.text().to_string())
                    .or_default()
                    .push(NamedItem::FuzzyVariable(v));
            }
        }
    };
    if let Some(s) = system.inputs() {
        add_variables(s.variables().collect(), s.fuzzy_variables().collect());
    }
    if let Some(s) = system.outputs() {
        add_variables(s.variables().collect(), s.fuzzy_variables().collect());
    }
    if let Some(s) = system.locals() {
        add_variables(s.variables().collect(), s.fuzzy_variables().collect());
    }
    if let Some(def) = system.define_section() {
        for obj in def.objects() {
            if let Some(name) = obj.name() {
                items
                    .entry(name.text().to_string())
                    .or_default()
                    .push(NamedItem::Object(obj));
            }
        }
    }
    for (name, group) in &items {
        if group.len() > 1 {
            diags.extend(group.iter().map(|item| item.diagnostic(name)));
        }
    }

    // 2. Duplicate equation targets within each SystemBody. `UseEquation`s writing to the same
    // variable are legal on their own (their instantiation domains only need to be disjoint —
    // checked elsewhere); only report when at least one conflicting definition is a
    // `StandardEquation` (source system: "skip conflicts only within UseEquations").
    for body in system.bodies() {
        let mut eqs: HashMap<String, Vec<Equation>> = HashMap::new();
        for eq in body.equations() {
            match &eq {
                Equation::Standard(s) => {
                    if let Some(name) = s.variable_name() {
                        eqs.entry(name.text().to_string())
                            .or_default()
                            .push(eq.clone());
                    }
                }
                Equation::Use(u) => {
                    for out in u.output_exprs() {
                        let mut names = Vec::new();
                        collect_variable_names(&out, &mut names);
                        for name in names {
                            eqs.entry(name).or_default().push(eq.clone());
                        }
                    }
                }
            }
        }
        for (name, group) in &eqs {
            if group.len() <= 1 || group.iter().all(|e| matches!(e, Equation::Use(_))) {
                continue;
            }
            for eq in group {
                let (start, end) = match eq {
                    Equation::Standard(s) => s
                        .variable_name()
                        .map(|t| token_range(&t))
                        .unwrap_or_else(|| range_of(s.syntax())),
                    Equation::Use(u) => range_of(u.syntax()),
                };
                diags.push(match eq {
                    Equation::Standard(_) => Diagnostic::DuplicateStandardEquation {
                        name: name.clone(),
                        start,
                        end,
                    },
                    Equation::Use(_) => Diagnostic::DuplicateUseEquation {
                        name: name.clone(),
                        start,
                        end,
                    },
                });
            }
        }
    }

    // 3. Duplicate `constant` declarations visible to this system.
    let mut consts: HashMap<String, Vec<ast::AlphaConstant>> = HashMap::new();
    for c in direct_container_constants(system) {
        if let Some(name) = c.name() {
            consts.entry(name.text().to_string()).or_default().push(c);
        }
    }
    for (name, group) in &consts {
        if group.len() > 1 {
            for c in group {
                let (start, end) = c
                    .name()
                    .map(|t| token_range(&t))
                    .unwrap_or_else(|| range_of(c.syntax()));
                diags.push(Diagnostic::DuplicateAlphaConstant {
                    name: name.clone(),
                    start,
                    end,
                });
            }
        }
    }

    diags
}

/// `prefix` (a package's dotted segments) joined with `name` — the fully-qualified name used to
/// key [`check_program_uniqueness`]'s duplicate-system/external-function maps. Not required to
/// match the source system's Xtext-generated qualified-name format byte-for-byte, only to be a
/// canonical, collision-correct key.
fn fqn(prefix: &[String], name: &str) -> String {
    let mut segments = prefix.to_vec();
    segments.push(name.to_string());
    segments.join(".")
}

#[derive(Default)]
struct Collected {
    systems: Vec<(String, ast::System)>,
    external_functions: Vec<(String, ast::ExternalFunction)>,
}

fn collect(
    prefix: &[String],
    systems: impl Iterator<Item = ast::System>,
    external_functions: impl Iterator<Item = ast::ExternalFunction>,
    packages: impl Iterator<Item = ast::AlphaPackage>,
    out: &mut Collected,
) {
    for s in systems {
        let name = s.name().map(|t| t.text().to_string()).unwrap_or_default();
        out.systems.push((fqn(prefix, &name), s));
    }
    for ef in external_functions {
        let name = ef.name().map(|t| t.text().to_string()).unwrap_or_default();
        out.external_functions.push((fqn(prefix, &name), ef));
    }
    for pkg in packages {
        let mut sub_prefix = prefix.to_vec();
        if let Some(qn) = pkg.qualified_name() {
            sub_prefix.extend(qn.segments().map(|t| t.text().to_string()));
        }
        collect(
            &sub_prefix,
            pkg.systems(),
            pkg.external_functions(),
            pkg.packages(),
            out,
        );
    }
}

/// Program-wide name-uniqueness checks (§6, phase 5): duplicate systems/external functions by
/// fully-qualified name across every `Root` given, plus [`check_system_uniqueness`] folded in for
/// every system found — mirrors the source system's `check(List<AlphaRoot>)`, which composes the
/// two the same way.
pub fn check_program_uniqueness(roots: &[ast::Root]) -> Vec<Diagnostic> {
    let mut collected = Collected::default();
    for root in roots {
        collect(
            &[],
            root.systems(),
            root.external_functions(),
            root.packages(),
            &mut collected,
        );
    }

    let mut diags = Vec::new();

    let mut by_name: HashMap<&str, Vec<&(String, ast::System)>> = HashMap::new();
    for item in &collected.systems {
        by_name.entry(item.0.as_str()).or_default().push(item);
    }
    for group in by_name.values().filter(|g| g.len() > 1) {
        for (_, s) in group {
            let (start, end) = s
                .name()
                .map(|t| token_range(&t))
                .unwrap_or_else(|| range_of(s.syntax()));
            diags.push(Diagnostic::DuplicateSystem {
                name: s.name().map(|t| t.text().to_string()).unwrap_or_default(),
                start,
                end,
            });
        }
    }

    let mut ef_by_name: HashMap<&str, Vec<&(String, ast::ExternalFunction)>> = HashMap::new();
    for item in &collected.external_functions {
        ef_by_name.entry(item.0.as_str()).or_default().push(item);
    }
    for group in ef_by_name.values().filter(|g| g.len() > 1) {
        for (_, ef) in group {
            let (start, end) = ef
                .name()
                .map(|t| token_range(&t))
                .unwrap_or_else(|| range_of(ef.syntax()));
            diags.push(Diagnostic::DuplicateExternalFunction {
                name: ef.name().map(|t| t.text().to_string()).unwrap_or_default(),
                start,
                end,
            });
        }
    }

    for (_, s) in &collected.systems {
        diags.extend(check_system_uniqueness(s));
    }

    diags
}
