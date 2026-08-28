//! Phase 1 of the six-phase pipeline: resolve each system's
//! parameter domain and every variable's domain into real isl objects — the "interface" pass
//! (`DOMAIN_CALC_MODE::INTERFACE_ONLY` in the source Java), before any equation body is looked
//! at. Phase 2 (resolving calculator expressions *inside* equation bodies, which need per-
//! equation index-name context) is a separate, later step — see this module's doc note on why
//! that split matters.
//!
//! `DefinedObject`/`VariableDomain` references are resolved lazily with cycle detection
//! (mirrors the source system's `z__internalCycleDetector`/`CyclicDefinitionException`): looking
//! up a variable's or defined-object's value triggers resolving it on first access, memoizing
//! the result, and reports [`Diagnostic::CyclicDefinition`] if resolution re-enters itself.

use crate::diagnostic::Diagnostic;
use crate::value::{eval_binary, eval_unary, Value};
use crate::{Multiplicity, VariableId};
use alpha_syntax::ast::{self, AstNode, CalcExpr};
use alpha_syntax::syntax_kind::SyntaxNode;
use isl::{Context, Set};
use std::collections::HashMap;

fn range_of(node: &SyntaxNode) -> (u32, u32) {
    let r = node.text_range();
    (r.start().into(), r.end().into())
}

/// Alpha's `{}` empty-domain shorthand (sugar for the 0-dimensional universe, per the source
/// grammar/`AlphaCustomValueConverter`) isn't valid on its own to isl's parser — confirmed
/// empirically: `isl_set_read_from_str("{}")` fails an internal assertion (isl seems to parse a
/// bare `{}` as some other polymorphic object kind, not specifically a set), while `{ : }` parses
/// fine as the same 0-d universe. Mirrors the source Java's substitution of `{}` to `"{ [] : }"`.
fn normalize_domain_text(text: &str) -> String {
    if text.trim() == "{}" {
        "{ [] : }".to_string()
    } else {
        text.to_string()
    }
}

fn isl_err(e: isl::IslError, node: &SyntaxNode) -> Diagnostic {
    let (start, end) = range_of(node);
    Diagnostic::IslError {
        message: e.message,
        start,
        end,
    }
}

/// Every `constant NAME = INT` declaration in scope for a system — found by walking up from the
/// system's own syntax node through its enclosing `AlphaPackage`/`Root` ancestors and collecting
/// each one's direct `AlphaConstant` children (Alpha's constants are declared alongside systems
/// in the same package/root, not nested inside them).
fn constants_in_scope(system: &ast::System) -> HashMap<String, i64> {
    let mut out = HashMap::new();
    let mut ancestor = system.syntax().parent();
    while let Some(node) = ancestor {
        for child in node.children() {
            if let Some(c) = ast::AlphaConstant::cast(child) {
                if let (Some(name), Some(value)) = (c.name(), c.value()) {
                    if let Ok(v) = value.text().parse::<i64>() {
                        out.insert(name.text().to_string(), v);
                    }
                }
            }
        }
        ancestor = node.parent();
    }
    out
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    InProgress,
    Done,
}

/// Resolves a single system's interface: its parameter domain, and every input/output/local
/// variable's domain (with comma-list inheritance — see [`Self::variable_domain`]'s doc). One
/// `Resolver` per system: `define`d objects and `{X}` variable-domain references are scoped to
/// the system they're declared in, so a fresh cache per system is both correct and simplest.
pub struct Resolver<'a> {
    pub(crate) ctx: Context,
    system: &'a ast::System,
    param_names: Vec<String>,
    constants: HashMap<String, i64>,
    defined_objects: HashMap<String, ast::PolyhedralObject>,
    defined_cache: HashMap<String, Value>,
    defined_state: HashMap<String, State>,
    /// Keyed by variable name; the fully-specified sibling's own text drives resolution for
    /// bare names in the same comma-group (see [`Self::variable_domain`]).
    variables_by_name: HashMap<String, ast::Variable>,
    variable_ids: HashMap<String, VariableId>,
    variable_multiplicities: HashMap<String, Multiplicity>,
    variable_cache: HashMap<String, Set>,
    variable_state: HashMap<String, State>,
}

impl<'a> Resolver<'a> {
    pub fn new(ctx: Context, system: &'a ast::System) -> Self {
        let mut defined_objects = HashMap::new();
        if let Some(section) = system.define_section() {
            for obj in section.objects() {
                if let Some(name) = obj.name() {
                    defined_objects.insert(name.text().to_string(), obj);
                }
            }
        }
        let mut variables_by_name = HashMap::new();
        let mut variable_ids = HashMap::new();
        let mut variable_multiplicities = HashMap::new();
        let mut next_variable_id = 0;
        for section_vars in [
            system.inputs().map(|s| s.variables().collect::<Vec<_>>()),
            system.outputs().map(|s| s.variables().collect::<Vec<_>>()),
            system.locals().map(|s| s.variables().collect::<Vec<_>>()),
        ]
        .into_iter()
        .flatten()
        {
            let mut group_multiplicity = Multiplicity::Unrestricted;
            for v in section_vars {
                if let Some(name) = v.name() {
                    if v.is_linear() {
                        group_multiplicity = Multiplicity::Linear;
                    }
                    let name = name.text().to_string();
                    variable_ids.insert(name.clone(), VariableId::from_index(next_variable_id));
                    variable_multiplicities.insert(name.clone(), group_multiplicity);
                    next_variable_id += 1;
                    let ends_group = v.domain().is_some();
                    variables_by_name.insert(name, v);
                    if ends_group {
                        group_multiplicity = Multiplicity::Unrestricted;
                    }
                }
            }
        }
        Resolver {
            ctx,
            system,
            param_names: system
                .param_domain()
                .map(|pd| pd.param_names().map(|t| t.text().to_string()).collect())
                .unwrap_or_default(),
            constants: constants_in_scope(system),
            defined_objects,
            defined_cache: HashMap::new(),
            defined_state: HashMap::new(),
            variables_by_name,
            variable_ids,
            variable_multiplicities,
            variable_cache: HashMap::new(),
            variable_state: HashMap::new(),
        }
    }

    /// The system this resolver was built for — needed by [`crate::domain`] to walk sibling
    /// `SystemBody`s when computing a body's own parameter domain (the `else` body's implicit
    /// domain depends on every other body's `when` guard).
    pub(crate) fn system(&self) -> &ast::System {
        self.system
    }

    pub fn variable_id(&self, name: &str) -> Option<VariableId> {
        self.variable_ids.get(name).copied()
    }

    pub fn variable_multiplicity(&self, name: &str) -> Option<Multiplicity> {
        self.variable_multiplicities.get(name).copied()
    }

    /// The system's declared parameter domain (`[N,M]->{:N>0 and M>0}` or bare `{:...}`) — this
    /// text is already valid isl parameter-set syntax verbatim, so no rewriting is needed before
    /// handing it to isl's own parser (unlike `ArrayDomain`/`RectangularDomain`, which need a
    /// parameter-list prefix synthesized — see [`Self::array_domain_in_param_context`]).
    pub fn param_domain(&self) -> Result<Set, Diagnostic> {
        let pd = self
            .system
            .param_domain()
            .expect("parser always produces a param domain for a well-formed System");
        Set::read_from_str(&self.ctx, &self.text_of(pd.syntax()))
            .map_err(|e| isl_err(e, pd.syntax()))
    }

    /// A `when`/`else` system-body guard's `{: constraints}` text — needs the system's
    /// parameter names prefixed (`[N,M] -> {: constraints}`) before isl can parse it, since bare
    /// `{: ...}` isn't valid isl syntax on its own (it needs to know what `N`/`M` are).
    pub fn array_domain_in_param_context(&self, ad: &ast::ArrayDomain) -> Result<Set, Diagnostic> {
        Set::read_from_str(
            &self.ctx,
            &self.with_param_prefix(&self.text_of(ad.syntax())),
        )
        .map_err(|e| isl_err(e, ad.syntax()))
    }

    /// Prepends the system's parameter names (`[N,M] -> ...`) to raw isl text that doesn't
    /// declare its own — required because isl's parser does *not* auto-infer free identifiers as
    /// parameters in a bare `{...}` literal (confirmed empirically: `{[i,j]:0<=i<N}` alone is a
    /// syntax error to isl, `[N]->{[i,j]:0<=i<N}` isn't). Every `CalculatorExpression` leaf
    /// except `ParamDomain` (which already carries its own optional prefix verbatim) needs this.
    pub(crate) fn with_param_prefix(&self, isl_text: &str) -> String {
        format!("[{}] -> {}", self.param_names.join(","), isl_text)
    }

    /// Renders a raw-captured calculator node's text, substituting any `AlphaConstant` reference
    /// (`constant factor=2` used as `factor*WW=W`) with its integer value first. isl has no
    /// notion of Alpha's named constants at all — the source Java system handles this with a
    /// blunt `String.replaceAll` *before* the text ever reaches isl, a fragile gotcha worth
    /// fixing properly; this does the same substitution but token-aware (only actual `IDENT`
    /// tokens are considered, never
    /// substring matches inside other identifiers or numbers) — the lexer/parser already knows
    /// where identifiers are, so substituting textually before constructing the isl-bound string
    /// is strictly safer than a post-hoc `replaceAll`.
    pub(crate) fn text_of(&self, node: &SyntaxNode) -> String {
        use alpha_syntax::syntax_kind::SyntaxKind;
        let mut out = String::new();
        for elem in node.children_with_tokens() {
            if let Some(t) = elem.as_token() {
                if t.kind() == SyntaxKind::IDENT {
                    match self.constants.get(t.text()) {
                        Some(v) => out.push_str(&v.to_string()),
                        None => out.push_str(t.text()),
                    }
                } else {
                    out.push_str(t.text());
                }
            } else if let Some(n) = elem.as_node() {
                out.push_str(&self.text_of(n));
            }
        }
        out
    }

    /// A variable's domain, resolving comma-list inheritance: a bare name (no `domain()` of its
    /// own — see `ast::Variable`'s doc) inherits the domain of the next sibling in its
    /// `inputs`/`outputs`/`locals` section that does have one, exactly as the source system's
    /// `JNIDomainCalculator.resolveVariableDeclaration` does. Memoized + cycle-detected since a
    /// variable's domain expression can itself reference other variables via `VariableDomain`
    /// (`{OtherVar}`).
    pub fn variable_domain(&mut self, name: &str) -> Result<Set, Diagnostic> {
        if let Some(s) = self.variable_cache.get(name) {
            return Ok(s.clone());
        }
        let (start, end) = self
            .variables_by_name
            .get(name)
            .map(|v| range_of(v.syntax()))
            .unwrap_or((0, 0));
        match self.variable_state.get(name) {
            Some(State::InProgress) => {
                return Err(Diagnostic::CyclicDefinition {
                    name: name.to_string(),
                    start,
                    end,
                })
            }
            Some(State::Done) => unreachable!("Done implies variable_cache already had it"),
            None => {}
        }
        let Some(var) = self.variables_by_name.get(name).cloned() else {
            return Err(Diagnostic::UndefinedReference {
                name: name.to_string(),
                start,
                end,
            });
        };
        self.variable_state
            .insert(name.to_string(), State::InProgress);
        // Deliberately not `?` here: on failure we must clear `InProgress` before returning, or
        // a later, unrelated lookup of this same name would see the stale state and wrongly
        // report `CyclicDefinition` for what was actually just an ordinary resolution error.
        let result = match var.domain() {
            Some(expr) => self.eval_domain_expr(&expr),
            None => {
                // Bare name in a comma-list: inherit from the next sibling with a domain. The
                // parser already grouped this correctly via `variable_clause`'s lookahead, so
                // the terminating sibling is always resolvable by scanning forward through the
                // same containing section.
                self.inherit_domain_from_next_sibling(&var)
            }
        };
        match result {
            Ok(value) => {
                self.variable_state.insert(name.to_string(), State::Done);
                self.variable_cache.insert(name.to_string(), value.clone());
                Ok(value)
            }
            Err(e) => {
                self.variable_state.remove(name);
                Err(e)
            }
        }
    }

    fn inherit_domain_from_next_sibling(&mut self, var: &ast::Variable) -> Result<Set, Diagnostic> {
        let mut sibling = var.syntax().next_sibling();
        while let Some(s) = sibling {
            if let Some(v) = ast::Variable::cast(s.clone()) {
                if v.domain().is_some() {
                    return self.variable_domain(v.name().expect("named variable").text());
                }
                sibling = s.next_sibling();
                continue;
            }
            break;
        }
        let (start, end) = range_of(var.syntax());
        Err(Diagnostic::UndefinedReference {
            name: var.name().map(|t| t.text().to_string()).unwrap_or_default(),
            start,
            end,
        })
    }

    fn eval_domain_expr(&mut self, expr: &CalcExpr) -> Result<Set, Diagnostic> {
        match self.eval_calc_expr(expr)? {
            Value::Set(s) => Ok(s),
            other => {
                let (start, end) = range_of(expr.syntax());
                Err(Diagnostic::InvalidCalculatorOperand {
                    operator: "domain position".to_string(),
                    operand_kind: other.kind_name().to_string(),
                    start,
                    end,
                })
            }
        }
    }

    /// Evaluates any `CalculatorExpression` in the "interface" context — i.e. no ambient
    /// equation-local index names are in scope, matching phase 1's scope (variable domains and
    /// `define`d objects referenced from them). Phase 2 (calculator expressions inside equation
    /// bodies, e.g. a `RestrictExpression`'s domain) needs per-equation index-name context this
    /// resolver doesn't thread yet — a deliberate scope boundary, not an oversight; see the
    /// module doc.
    pub fn eval_calc_expr(&mut self, expr: &CalcExpr) -> Result<Value, Diagnostic> {
        match expr {
            CalcExpr::Domain(d) => Ok(Value::Set(
                Set::read_from_str(
                    &self.ctx,
                    &self.with_param_prefix(&normalize_domain_text(&self.text_of(d.syntax()))),
                )
                .map_err(|e| isl_err(e, d.syntax()))?,
            )),
            CalcExpr::ArrayDomain(ad) => Ok(Value::Set(self.array_domain_in_param_context(ad)?)),
            CalcExpr::ParamDomain(pd) => Ok(Value::Set(
                // Already fully self-contained (its own optional `[params]->` prefix is part of
                // its own text verbatim) — unlike the other branches here, never re-prefix this
                // one, or a system that already declares `[N]->{...}` would end up doubly
                // parameter-qualified.
                Set::read_from_str(&self.ctx, &self.text_of(pd.syntax()))
                    .map_err(|e| isl_err(e, pd.syntax()))?,
            )),
            CalcExpr::Relation(r) => Ok(Value::Map(
                isl::Map::read_from_str(
                    &self.ctx,
                    &self.with_param_prefix(&self.text_of(r.syntax())),
                )
                .map_err(|e| isl_err(e, r.syntax()))?,
            )),
            CalcExpr::Polynomial(p) => Ok(Value::Polynomial(
                isl::PwQPolynomial::read_from_str(
                    &self.ctx,
                    &self.with_param_prefix(&self.text_of(p.syntax())),
                )
                .map_err(|e| isl_err(e, p.syntax()))?,
            )),
            CalcExpr::ArrayPolynomial(p) => Ok(Value::Polynomial(
                isl::PwQPolynomial::read_from_str(
                    &self.ctx,
                    &self.with_param_prefix(&self.text_of(p.syntax())),
                )
                .map_err(|e| isl_err(e, p.syntax()))?,
            )),
            CalcExpr::VariableDomain(vd) => {
                let name = vd.name().map(|t| t.text().to_string()).unwrap_or_default();
                Ok(Value::Set(self.variable_domain(&name)?))
            }
            CalcExpr::DefinedObject(obj) => {
                let name = obj.name().map(|t| t.text().to_string()).unwrap_or_default();
                self.defined_object(&name, obj.syntax())
            }
            CalcExpr::RectangularDomain(rect) => Ok(Value::Set(self.rectangular_domain(rect)?)),
            CalcExpr::Unary(u) => {
                let operand = u
                    .operand()
                    .map(|o| self.eval_calc_expr(&o))
                    .unwrap_or_else(|| {
                        let (start, end) = range_of(u.syntax());
                        Err(Diagnostic::UndefinedReference {
                            name: "<missing operand>".to_string(),
                            start,
                            end,
                        })
                    })?;
                eval_unary(u, operand)
            }
            CalcExpr::Binary(b) => {
                let lhs = self.require_operand(b.lhs(), b.syntax())?;
                let rhs = self.require_operand(b.rhs(), b.syntax())?;
                eval_binary(b, lhs, rhs)
            }
            CalcExpr::Paren(p) => self.require_operand(p.inner(), p.syntax()),
            CalcExpr::Function(_) | CalcExpr::ArrayFunction(_) => {
                // Function literals need ambient index-name context to resolve (see the module
                // doc) — reachable in phase-1-only positions (variable domains, `define`d
                // objects) only in unusual programs; report clearly rather than mis-evaluate.
                let (start, end) = range_of(expr.syntax());
                Err(Diagnostic::UnsupportedCalculatorOp {
                    operator: "function literal outside an equation body".to_string(),
                    start,
                    end,
                })
            }
            CalcExpr::FuzzyFunction(_) | CalcExpr::ArrayFuzzyFunction(_) => {
                let (start, end) = range_of(expr.syntax());
                Err(Diagnostic::UnsupportedCalculatorOp {
                    operator: "fuzzy function".to_string(),
                    start,
                    end,
                })
            }
        }
    }

    fn require_operand(
        &mut self,
        expr: Option<CalcExpr>,
        fallback_node: &SyntaxNode,
    ) -> Result<Value, Diagnostic> {
        match expr {
            Some(e) => self.eval_calc_expr(&e),
            None => {
                let (start, end) = range_of(fallback_node);
                Err(Diagnostic::UndefinedReference {
                    name: "<missing operand>".to_string(),
                    start,
                    end,
                })
            }
        }
    }

    fn defined_object(&mut self, name: &str, ref_node: &SyntaxNode) -> Result<Value, Diagnostic> {
        if let Some(v) = self.defined_cache.get(name) {
            return Ok(v.clone());
        }
        let (start, end) = range_of(ref_node);
        match self.defined_state.get(name) {
            Some(State::InProgress) => {
                return Err(Diagnostic::CyclicDefinition {
                    name: name.to_string(),
                    start,
                    end,
                })
            }
            Some(State::Done) => unreachable!("Done implies defined_cache already had it"),
            None => {}
        }
        let Some(obj) = self.defined_objects.get(name).cloned() else {
            return Err(Diagnostic::UndefinedReference {
                name: name.to_string(),
                start,
                end,
            });
        };
        self.defined_state
            .insert(name.to_string(), State::InProgress);
        // Same reasoning as `variable_domain`: clear `InProgress` on *any* failure path (missing
        // expression or a real eval error), not just success, or a later lookup of this name
        // would wrongly report `CyclicDefinition`.
        let result = obj
            .expr()
            .ok_or_else(|| Diagnostic::UndefinedReference {
                name: name.to_string(),
                start,
                end,
            })
            .and_then(|expr| self.eval_calc_expr(&expr));
        match result {
            Ok(value) => {
                self.defined_state.insert(name.to_string(), State::Done);
                self.defined_cache.insert(name.to_string(), value.clone());
                Ok(value)
            }
            Err(e) => {
                self.defined_state.remove(name);
                Err(e)
            }
        }
    }

    /// Expands `RectangularDomain` (`[N,N] as [i,j]` / `[0:N-1,...] as [...]`) into an explicit
    /// isl set: `[i,j] : 0<=i<N and 0<=j<N` for the upper-bounds-only form, or `[i,j] : l1<=i<u1
    /// and l2<=j<u2` for the lower:upper form. Bound expressions are split on top-level commas
    /// (they're raw `AISLExpression` text, but real programs don't nest commas inside a single
    /// bound, so a plain split is sufficient here — unlike the parser's own bracket-depth
    /// tracking, which has to handle arbitrary nesting).
    fn rectangular_domain(&self, rect: &ast::RectangularDomain) -> Result<Set, Diagnostic> {
        let text = self.text_of(rect.syntax());
        let inner = text
            .trim_start_matches('[')
            .split(']')
            .next()
            .unwrap_or("")
            .to_string();
        let bounds: Vec<&str> = inner.split(',').map(|s| s.trim()).collect();
        let names: Vec<String> = {
            let explicit: Vec<String> = rect.index_names().map(|t| t.text().to_string()).collect();
            if !explicit.is_empty() {
                explicit
            } else {
                (0..bounds.len()).map(|i| format!("__rect{i}")).collect()
            }
        };
        let mut constraints = Vec::with_capacity(bounds.len());
        for (bound, name) in bounds.iter().zip(&names) {
            if let Some((lo, hi)) = bound.split_once(':') {
                constraints.push(format!("{}<={}<{}", lo.trim(), name, hi.trim()));
            } else {
                constraints.push(format!("0<={}<{}", name, bound.trim()));
            }
        }
        let set_text = format!(
            "{{ [{}] : {} }}",
            names.join(","),
            constraints.join(" and ")
        );
        Set::read_from_str(&self.ctx, &self.with_param_prefix(&set_text))
            .map_err(|e| isl_err(e, rect.syntax()))
    }
}
