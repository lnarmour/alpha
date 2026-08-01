//! Phases 3–4 of the six-phase pipeline: bottom-up
//! expression-domain inference (`ExpressionDomainCalculator` in the source Java) and top-down
//! context-domain inference (`ContextDomainCalculator`).
//!
//! **Expression domain** (phase 3): the set of index tuples where an expression can be
//! evaluated at all, computed bottom-up from the leaves (constants over the system's scalar
//! domain, variables over their declared domain) through the usual combinators (binary/multi-arg
//! ops intersect their operands, `case` unions its branches, `f@e`/array notation takes the
//! pre-image of its operand under `f`, `reduce` takes the image of its body under the
//! projection).
//!
//! **Context domain** (phase 4): the set of index tuples where an expression's value is actually
//! *needed* to compute the equation's outputs, computed top-down starting from the equation's own
//! declared domain and narrowed at each step by intersecting with that node's own expression
//! domain, with the parent's context re-mapped into the child's index space at the handful of
//! constructs that change index space (`DependenceExpression`/`SelectExpression` map forward
//! through their function/relation, `ReduceExpression` maps backward through its projection).
//!
//! Both are threaded through the same ambient index-name context (`ArrayFunction`'s implicit
//! input tuple) that [`crate::function`] resolves — see that module's doc for the exact scoping
//! rules each construct follows; this module reuses [`crate::context_names`]'s helpers to extend
//! that context identically for both passes, so they never disagree about what's in scope where.
//!
//! **Known, deliberate gaps** (this project's practice throughout is to document scope
//! boundaries rather than silently under-handle them — see `alpha-codegen`'s own `UseEquation`
//! codegen gap for the precedent):
//! - `ConvolutionExpression`'s own expression/context domain needs vertex enumeration of the
//!   kernel domain (`AlphaExpressionUtil.preimageByConvolutionDependences` in the source system),
//!   which isl exposes but this crate's bounded `isl-sys` surface doesn't bind yet (§5). Reports
//!   [`Diagnostic::UnsupportedCalculatorOp`] for the convolution node itself; its kernel/data
//!   sub-expressions are still walked and recorded normally.
//! - `UseEquation`'s context domain needs the cross-system instantiation-domain extension
//!   (`AlphaExpressionUtil.extendCalleeDomainByInstantiationDomain`) — genuinely more involved
//!   than anything else here (it reasons about the *callee* system's own domains). Only
//!   `StandardEquation` bodies get phase 4 for now; `UseEquation` bodies get phase 3 only.

use crate::context_names::{
    bare_identifier_elements, domain_tuple_names, extend_unique, relation_range_names,
};
use crate::diagnostic::Diagnostic;
use crate::resolve::Resolver;
use crate::value::Value;
use alpha_syntax::ast::{self, AstNode, CalcExpr, Equation, Expr};
use alpha_syntax::syntax_kind::SyntaxNode;
use isl::{DimType, Map, MultiAff, Set};
use std::collections::HashMap;

/// Every `Expr` node's inferred domain (expression or context, depending on which map this is),
/// keyed by its syntax node. One node's expression domain and context domain are always kept in
/// separate maps (a node needs both, and they're computed in separate passes).
pub type Domains = HashMap<SyntaxNode, Set>;

fn range_of(node: &SyntaxNode) -> (u32, u32) {
    let r = node.text_range();
    (r.start().into(), r.end().into())
}

fn isl_err(e: isl::IslError, node: &SyntaxNode) -> Diagnostic {
    let (start, end) = range_of(node);
    Diagnostic::IslError {
        message: e.message,
        start,
        end,
    }
}

fn missing(what: &str, node: &SyntaxNode) -> Diagnostic {
    let (start, end) = range_of(node);
    Diagnostic::UndefinedReference {
        name: format!("<missing {what}>"),
        start,
        end,
    }
}

/// `UseEquation`'s own ambient index-name context: the `over` clause's own index tuple *and* the
/// `with` clause's names are both in scope for its output/input expressions (e.g. FFT's `over[2]
/// as [k] with [i] : (q[k,i]) = FFT[N/2](p[k,i])` needs both `k` and `i`).
pub fn use_equation_context(u: &ast::UseEquation) -> Vec<String> {
    let over_names = match u.instantiation_domain() {
        Some(CalcExpr::RectangularDomain(rect)) => {
            rect.index_names().map(|t| t.text().to_string()).collect()
        }
        Some(CalcExpr::Domain(d)) => domain_tuple_names(&d),
        _ => Vec::new(),
    };
    extend_unique(
        &over_names,
        u.subsystem_dims().map(|t| t.text().to_string()),
    )
}

impl Resolver<'_> {
    /// `AlphaUtil.getScalarDomain`: the 0-dimensional universe, intersected with the system's own
    /// parameter constraints — what a bare constant (`ConstantExpression`: `Bool`/`Int`/`Real`
    /// literals here) is defined over.
    pub fn scalar_domain(&self) -> Result<Set, Diagnostic> {
        let pdom = self.param_domain()?;
        let universe = Set::read_from_str(&self.ctx, &self.with_param_prefix("{ [] : }"))
            .map_err(|e| isl_err(e, self.system().syntax()))?;
        universe
            .intersect_params(pdom)
            .map_err(|e| isl_err(e, self.system().syntax()))
    }

    /// A `SystemBody`'s own parameter domain: its `when` guard, evaluated in parameter context —
    /// or, for the one body allowed to omit `when` (the implicit `else`), the system's own
    /// parameter domain minus the union of every other body's guard (`JNIDomainCalculator`'s
    /// `completeSystemBody`).
    pub fn system_body_domain(&self, body: &ast::SystemBody) -> Result<Set, Diagnostic> {
        if let Some(guard) = body.when_domain() {
            return self.array_domain_in_param_context(&guard);
        }
        let mut others_without_guard = 0u32;
        let mut union: Option<Set> = None;
        for b in self.system().bodies() {
            match b.when_domain() {
                Some(guard) => {
                    let d = self.array_domain_in_param_context(&guard)?;
                    union = Some(match union {
                        None => d,
                        Some(u) => u.union(d).map_err(|e| isl_err(e, body.syntax()))?,
                    });
                }
                None => others_without_guard += 1,
            }
        }
        if others_without_guard > 1 {
            let (start, end) = range_of(body.syntax());
            return Err(Diagnostic::MultipleUnrestrictedSystemBody { start, end });
        }
        match union {
            None => self.param_domain(),
            Some(u) => self
                .param_domain()?
                .subtract(u)
                .map_err(|e| isl_err(e, body.syntax())),
        }
    }

    /// Phase 3: the expression domain of `expr` and everything beneath it, recorded into
    /// `domains`. `context` is the ambient index-name context in scope at this point (see the
    /// module doc).
    pub fn expression_domain(
        &mut self,
        expr: &Expr,
        context: &[String],
        domains: &mut Domains,
    ) -> Result<Set, Diagnostic> {
        let domain = self.expression_domain_uncached(expr, context, domains)?;
        domains.insert(expr.syntax().clone(), domain.clone());
        Ok(domain)
    }

    fn expression_domain_uncached(
        &mut self,
        expr: &Expr,
        context: &[String],
        domains: &mut Domains,
    ) -> Result<Set, Diagnostic> {
        match expr {
            Expr::Bool(_) | Expr::Int(_) | Expr::Real(_) => self.scalar_domain(),
            Expr::Variable(v) => {
                let name = v.name().map(|t| t.text().to_string()).unwrap_or_default();
                self.variable_domain(&name)
            }
            Expr::Binary(b) => {
                let l = self.expression_domain(&require(b.lhs(), b.syntax())?, context, domains)?;
                let r = self.expression_domain(&require(b.rhs(), b.syntax())?, context, domains)?;
                l.intersect(r).map_err(|e| isl_err(e, expr.syntax()))
            }
            Expr::Unary(u) => {
                self.expression_domain(&require(u.operand(), u.syntax())?, context, domains)
            }
            Expr::Paren(p) => {
                self.expression_domain(&require(p.inner(), p.syntax())?, context, domains)
            }
            Expr::MultiArg(m) => {
                let mut args = m.args();
                let first = args.next().ok_or_else(|| missing("argument", m.syntax()))?;
                let mut acc = self.expression_domain(&first, context, domains)?;
                for a in args {
                    let d = self.expression_domain(&a, context, domains)?;
                    acc = acc.intersect(d).map_err(|e| isl_err(e, expr.syntax()))?;
                }
                Ok(acc)
            }
            Expr::Case(c) => {
                let mut branches = c.branches();
                let first = branches
                    .next()
                    .ok_or_else(|| missing("case branch", c.syntax()))?;
                let mut acc = self.expression_domain(&first, context, domains)?;
                for b in branches {
                    let d = self.expression_domain(&b, context, domains)?;
                    acc = acc.union(d).map_err(|e| isl_err(e, expr.syntax()))?;
                }
                Ok(acc)
            }
            Expr::AutoRestrict(a) => {
                self.expression_domain(&require(a.expr(), a.syntax())?, context, domains)
            }
            Expr::If(i) => {
                let cond_dom =
                    self.expression_domain(&require(i.cond(), i.syntax())?, context, domains)?;
                let then_dom = self.expression_domain(
                    &require(i.then_branch(), i.syntax())?,
                    context,
                    domains,
                )?;
                let else_dom = self.expression_domain(
                    &require(i.else_branch(), i.syntax())?,
                    context,
                    domains,
                )?;
                cond_dom
                    .intersect(then_dom)
                    .and_then(|s| s.intersect(else_dom))
                    .map_err(|e| isl_err(e, expr.syntax()))
            }
            Expr::Restrict(r) => {
                let (dom, extended) = self.restrict_domain(r, context)?;
                let inner =
                    self.expression_domain(&require(r.expr(), r.syntax())?, &extended, domains)?;
                inner.intersect(dom).map_err(|e| isl_err(e, expr.syntax()))
            }
            Expr::Dependence(d) => {
                let inner_expr = require(d.applied_expr(), d.syntax())?;
                let inner_domain = self.expression_domain(&inner_expr, context, domains)?;
                let f = d
                    .function()
                    .ok_or_else(|| missing("dependence function", d.syntax()))?;
                let maff = self.eval_function(&f, context)?;
                if maff.n_out() != inner_domain.dim(DimType::OutOrSet) {
                    let (start, end) = range_of(d.syntax());
                    return Err(Diagnostic::IncompatibleContextAndExpressionDomain { start, end });
                }
                inner_domain
                    .preimage_multi_aff(maff)
                    .map_err(|e| isl_err(e, expr.syntax()))
            }
            Expr::Index(ie) => self.index_expr_domain(ie, context),
            Expr::Reduce(r) => {
                let (proj, body_context) = self.reduce_projection(r, context)?;
                let body = require(r.body(), r.syntax())?;
                let body_domain = self.expression_domain(&body, &body_context, domains)?;
                body_domain
                    .apply(proj.into_map().map_err(|e| isl_err(e, expr.syntax()))?)
                    .map_err(|e| isl_err(e, expr.syntax()))
            }
            Expr::Select(s) => {
                let (rel, extended) = self.select_relation(s, context)?;
                let inner =
                    self.expression_domain(&require(s.expr(), s.syntax())?, &extended, domains)?;
                inner
                    .apply(rel.reverse().map_err(|e| isl_err(e, expr.syntax()))?)
                    .map_err(|e| isl_err(e, expr.syntax()))
            }
            Expr::Convolution(c) => {
                // Still walk (and record) the sub-expressions' own domains — only the
                // convolution node's *own* domain is the unbound piece (see module doc).
                let extended = self.convolution_kernel_names(c, context)?;
                if let Some(k) = c.kernel_expr() {
                    self.expression_domain(&k, &extended, domains)?;
                }
                if let Some(d) = c.data_expr() {
                    self.expression_domain(&d, &extended, domains)?;
                }
                let (start, end) = range_of(c.syntax());
                Err(Diagnostic::UnsupportedCalculatorOp {
                    operator: "convolution expression-domain inference".to_string(),
                    start,
                    end,
                })
            }
        }
    }

    pub fn index_expr_domain(
        &mut self,
        ie: &ast::IndexExpr,
        context: &[String],
    ) -> Result<Set, Diagnostic> {
        let src = ie
            .source()
            .ok_or_else(|| missing("index source", ie.syntax()))?;
        match &src {
            CalcExpr::Function(_) | CalcExpr::ArrayFunction(_) => {
                let maff = self.eval_function(&src, context)?;
                Set::universe(maff.domain_space()).map_err(|e| isl_err(e, ie.syntax()))
            }
            CalcExpr::Polynomial(_) => match self.eval_calc_expr(&src)? {
                Value::Polynomial(p) => {
                    Set::universe(p.domain_space()).map_err(|e| isl_err(e, ie.syntax()))
                }
                other => Err(Diagnostic::InvalidCalculatorOperand {
                    operator: "val index source".to_string(),
                    operand_kind: other.kind_name().to_string(),
                    start: range_of(ie.syntax()).0,
                    end: range_of(ie.syntax()).1,
                }),
            },
            CalcExpr::ArrayPolynomial(ap) => {
                let poly = self.eval_polynomial_in_context(ap, context)?;
                Set::universe(poly.domain_space()).map_err(|e| isl_err(e, ie.syntax()))
            }
            CalcExpr::FuzzyFunction(_) | CalcExpr::ArrayFuzzyFunction(_) => {
                let (start, end) = range_of(ie.syntax());
                Err(Diagnostic::UnsupportedCalculatorOp {
                    operator: "fuzzy index expression".to_string(),
                    start,
                    end,
                })
            }
            other => {
                let (start, end) = range_of(other.syntax());
                Err(Diagnostic::InvalidCalculatorOperand {
                    operator: "index source".to_string(),
                    operand_kind: "non-function, non-polynomial calculator expression".to_string(),
                    start,
                    end,
                })
            }
        }
    }

    /// `RestrictExpression`'s domain source, plus the index-name context for the restricted
    /// sub-expression. Two shapes, per the source system's `inRestrictExpression` (§6):
    /// - A bare `{: constraints}` (parsed as either node kind — see [`is_bare_colon_domain`]) has
    ///   no tuple of its own: its constraints refer directly to the *already ambient* index names
    ///   (e.g. `{:i=0}: A[i,j]`), so it's evaluated with the ambient context as its implicit
    ///   tuple, and the context passed to the sub-expression is unchanged.
    /// - An explicit-tuple `{[x,y]:...}` is self-declaring, evaluated context-free — and, once
    ///   its dimension count matches the ambient context's, its own tuple names *replace* the
    ///   context for the sub-expression (not extend it — a restrict's declared tuple is the
    ///   sub-expression's *entire* index space, discarding whatever names aren't part of it;
    ///   confirmed against `array1e`/`array2` in the real fixture corpus, where the restrict's
    ///   own tuple uses a name distinct from the enclosing equation's).
    pub fn restrict_domain(
        &mut self,
        r: &ast::RestrictExpr,
        context: &[String],
    ) -> Result<(Set, Vec<String>), Diagnostic> {
        let dom_calc = r
            .domain_source()
            .ok_or_else(|| missing("restrict domain", r.syntax()))?;
        let text = self.text_of(dom_calc.syntax());
        if matches!(&dom_calc, CalcExpr::ArrayDomain(_)) || is_bare_colon_domain(&text) {
            let s = self.constraints_in_index_context(dom_calc.syntax(), &text, context)?;
            return Ok((s, context.to_vec()));
        }
        let names = match &dom_calc {
            CalcExpr::Domain(d) => domain_tuple_names(d),
            _ => Vec::new(),
        };
        match self.eval_calc_expr(&dom_calc)? {
            Value::Set(s) => {
                if !names.is_empty() && !context.is_empty() && names.len() != context.len() {
                    let (start, end) = range_of(r.syntax());
                    return Err(Diagnostic::RestrictDomainDimensionMismatch { start, end });
                }
                let new_context = if names.is_empty() {
                    context.to_vec()
                } else {
                    names
                };
                Ok((s, new_context))
            }
            other => {
                let (start, end) = range_of(dom_calc.syntax());
                Err(Diagnostic::InvalidCalculatorOperand {
                    operator: "restrict domain".to_string(),
                    operand_kind: other.kind_name().to_string(),
                    start,
                    end,
                })
            }
        }
    }

    /// Evaluates a bare `{: constraints}` domain (regardless of whether the syntax layer tagged
    /// it `Domain` or `ArrayDomain` — see [`is_bare_colon_domain`]) using the ambient index-name
    /// context as its implicit tuple — `{ [ctx..] : constraints }` — rather than
    /// [`Resolver::array_domain_in_param_context`]'s parameter-only reading (used for `when`/
    /// `else` guards, where bare `{:...}` names are genuinely parameters, not indices).
    pub fn constraints_in_index_context(
        &self,
        node: &SyntaxNode,
        text: &str,
        context: &[String],
    ) -> Result<Set, Diagnostic> {
        let body = text
            .trim()
            .strip_prefix('{')
            .and_then(|s| s.strip_suffix('}'))
            .unwrap_or("")
            .trim();
        let full = format!("{{ [{}] {} }}", context.join(","), body);
        Set::read_from_str(&self.ctx, &self.with_param_prefix(&full)).map_err(|e| isl_err(e, node))
    }

    /// `SelectExpression`'s relation, plus the index-name context for the selected
    /// sub-expression. Per the source system's `inSelectExpression` (§6): once the relation's
    /// domain-side dimension count matches the ambient context's, its *range*-side tuple names
    /// *replace* the context for the sub-expression (not extend it — same reasoning as
    /// [`Self::restrict_domain`]'s explicit-tuple case).
    pub fn select_relation(
        &mut self,
        s: &ast::SelectExpr,
        context: &[String],
    ) -> Result<(Map, Vec<String>), Diagnostic> {
        let rel_calc = s
            .relation()
            .ok_or_else(|| missing("select relation", s.syntax()))?;
        let range_names = match &rel_calc {
            CalcExpr::Relation(r) => relation_range_names(r),
            _ => Vec::new(),
        };
        match self.eval_calc_expr(&rel_calc)? {
            Value::Map(m) => {
                if !context.is_empty() && m.dim(DimType::In) as usize != context.len() {
                    let (start, end) = range_of(s.syntax());
                    return Err(Diagnostic::SelectRelationDimensionMismatch { start, end });
                }
                let new_context = if range_names.is_empty() {
                    context.to_vec()
                } else {
                    range_names
                };
                Ok((m, new_context))
            }
            other => {
                let (start, end) = range_of(rel_calc.syntax());
                Err(Diagnostic::InvalidCalculatorOperand {
                    operator: "select relation".to_string(),
                    operand_kind: other.kind_name().to_string(),
                    start,
                    end,
                })
            }
        }
    }

    /// `ConvolutionExpression`'s kernel domain's own bound names (`[2] as [x]`), added to the
    /// ambient context for both its kernel weight and data sub-expressions.
    pub fn convolution_kernel_names(
        &mut self,
        c: &ast::ConvolutionExpr,
        context: &[String],
    ) -> Result<Vec<String>, Diagnostic> {
        match c.kernel_domain() {
            Some(CalcExpr::RectangularDomain(rect)) => Ok(extend_unique(
                context,
                rect.index_names().map(|t| t.text().to_string()),
            )),
            _ => Ok(context.to_vec()),
        }
    }

    /// `ReduceExpression`/`ArgReduceExpression`'s projection function, plus the index-name
    /// context in scope for its body. Two shapes, per the source system's
    /// `parseJNIFunctionAsProjection`/`JNIDomainCalculator.inAbstractReduceExpression`:
    /// - Bare `[k,...]` (`ArrayFunction`) sugar: the projection is `(ctx,k,... -> ctx)` (project
    ///   the new names back out), and the body sees `ctx` extended with the new names.
    /// - A full `(i,j -> ...)` (`Function`): self-declaring — the projection *is* that function
    ///   verbatim, and the body's context is exactly its own input names (replacing, not
    ///   extending, the ambient context).
    pub fn reduce_projection(
        &self,
        r: &ast::ReduceExpr,
        context: &[String],
    ) -> Result<(MultiAff, Vec<String>), Diagnostic> {
        let proj = r
            .projection()
            .ok_or_else(|| missing("reduce projection", r.syntax()))?;
        match &proj {
            CalcExpr::ArrayFunction(af) => {
                let names = bare_identifier_elements(af);
                let extended = extend_unique(context, names);
                let text = format!("{{ [{}] -> [{}] }}", extended.join(","), context.join(","));
                let maff = MultiAff::read_from_str(&self.ctx, &self.with_param_prefix(&text))
                    .map_err(|e| isl_err(e, af.syntax()))?;
                Ok((maff, extended))
            }
            CalcExpr::Function(f) => {
                let inputs: Vec<String> = f.index_names().map(|t| t.text().to_string()).collect();
                let maff = self.eval_function(&proj, &[])?;
                Ok((maff, inputs))
            }
            other => {
                let (start, end) = range_of(other.syntax());
                Err(Diagnostic::InvalidCalculatorOperand {
                    operator: "reduce projection".to_string(),
                    operand_kind: "non-function calculator expression".to_string(),
                    start,
                    end,
                })
            }
        }
    }

    /// Phase 3, driven for a whole equation body: `StandardEquation`'s expression under its own
    /// `[i,j]` index names, or a `UseEquation`'s output/input expressions under its `over`/`with`
    /// names (see [`use_equation_context`]).
    pub fn equation_expression_domains(
        &mut self,
        eq: &Equation,
        domains: &mut Domains,
    ) -> Result<(), Diagnostic> {
        match eq {
            Equation::Standard(s) => {
                let context: Vec<String> = s.index_names().map(|t| t.text().to_string()).collect();
                if let Some(expr) = s.expr() {
                    self.expression_domain(&expr, &context, domains)?;
                }
            }
            Equation::Use(u) => {
                let context = use_equation_context(u);
                for e in u.output_exprs().chain(u.input_exprs()) {
                    self.expression_domain(&e, &context, domains)?;
                }
            }
        }
        Ok(())
    }

    /// Phase 4, driven for a `StandardEquation`: its own context domain is its variable's
    /// declared domain, intersected with the enclosing `SystemBody`'s parameter domain
    /// (`AlphaExpressionUtil.parentContext`'s `StandardEquation` case) — everything beneath it is
    /// narrowed from there. `domains` must already hold this equation's expression domains (see
    /// [`Self::equation_expression_domains`]).
    pub fn equation_context_domains(
        &mut self,
        eq: &ast::StandardEquation,
        body: &ast::SystemBody,
        domains: &Domains,
        contexts: &mut Domains,
    ) -> Result<(), Diagnostic> {
        let Some(var_name) = eq.variable_name() else {
            return Ok(());
        };
        let var_domain = self.variable_domain(var_name.text())?;
        let body_domain = self.system_body_domain(body)?;
        // `intersect_params`, not `intersect`: `body_domain` is 0-dimensional (parameter
        // constraints only) while `var_domain` has the variable's own set dims — mirrors the
        // source system's `parent.variable.domain.intersectParams(parent.systemBody.parameterDomain)`.
        let own_context = var_domain
            .intersect_params(body_domain)
            .map_err(|e| isl_err(e, eq.syntax()))?;
        let context_names: Vec<String> = eq.index_names().map(|t| t.text().to_string()).collect();
        if let Some(expr) = eq.expr() {
            self.context_domain(&expr, own_context, &context_names, domains, contexts)?;
        }
        Ok(())
    }

    /// Runs phases 3–4 over every equation in the whole system, aggregating every equation's
    /// expression/context domains into one shared pair of maps (unlike
    /// [`Self::equation_expression_domains`]/[`Self::equation_context_domains`], which each work
    /// on one equation in isolation) — phase 6's whole-system checks (overlapping `case`
    /// branches, unbounded reductions, `UseEquation` output completeness, ...) need domains from
    /// *every* equation available together, not siloed per call.
    ///
    /// Doesn't fail fast: one equation's domain-inference error is collected into the returned
    /// diagnostics and analysis continues with the rest, so a single mistake elsewhere in a large
    /// system doesn't prevent phase 6 from checking everything else.
    pub fn analyze_system(&mut self, system: &ast::System) -> (Domains, Domains, Vec<Diagnostic>) {
        let mut domains = Domains::new();
        let mut contexts = Domains::new();
        let mut diagnostics = Vec::new();

        for body in system.bodies() {
            for eq in body.equations() {
                if let Err(d) = self.equation_expression_domains(&eq, &mut domains) {
                    diagnostics.push(d);
                    continue;
                }
                if let Equation::Standard(s) = &eq {
                    if let Err(d) = self.equation_context_domains(s, &body, &domains, &mut contexts)
                    {
                        diagnostics.push(d);
                    }
                }
            }
        }

        (domains, contexts, diagnostics)
    }

    /// Phase 4's recursive core: `parent_context` is already this node's *own* context (computed
    /// by whoever called this, per the module doc) — this both records it and, for constructs
    /// that change index space, transforms it before recursing into children.
    ///
    /// Deliberately does not pre-check `parent_context`/`expr`'s expression-domain spaces match
    /// before intersecting (unlike the source system's explicit check) — a real mismatch here
    /// surfaces as an ordinary [`Diagnostic::IslError`] from the failed `intersect` instead of the
    /// dedicated diagnostic, since isl's own space-compatibility check already does this for
    /// free and this crate's bound isl surface has no separate space-equality query (§5) to spend
    /// on duplicating it.
    fn context_domain(
        &mut self,
        expr: &Expr,
        parent_context: Set,
        context: &[String],
        domains: &Domains,
        contexts: &mut Domains,
    ) -> Result<(), Diagnostic> {
        if let Expr::AutoRestrict(are) = expr {
            return self.auto_restrict_context(are, parent_context, context, domains, contexts);
        }

        let Some(own_domain) = domains.get(expr.syntax()) else {
            // Mirrors the source system: if this node's expression domain was never computed
            // (an earlier error), gracefully skip rather than compounding the diagnostic.
            return Ok(());
        };
        let own_context = parent_context
            .intersect(own_domain.clone())
            .map_err(|e| isl_err(e, expr.syntax()))?;
        contexts.insert(expr.syntax().clone(), own_context.clone());

        match expr {
            Expr::Bool(_) | Expr::Int(_) | Expr::Real(_) | Expr::Variable(_) | Expr::Index(_) => {
                Ok(())
            }
            Expr::Binary(b) => {
                for sub in [b.lhs(), b.rhs()].into_iter().flatten() {
                    self.context_domain(&sub, own_context.clone(), context, domains, contexts)?;
                }
                Ok(())
            }
            Expr::Unary(u) => {
                if let Some(sub) = u.operand() {
                    self.context_domain(&sub, own_context, context, domains, contexts)?;
                }
                Ok(())
            }
            Expr::Paren(p) => {
                if let Some(sub) = p.inner() {
                    self.context_domain(&sub, own_context, context, domains, contexts)?;
                }
                Ok(())
            }
            Expr::MultiArg(m) => {
                for a in m.args() {
                    self.context_domain(&a, own_context.clone(), context, domains, contexts)?;
                }
                Ok(())
            }
            Expr::If(i) => {
                for sub in [i.cond(), i.then_branch(), i.else_branch()]
                    .into_iter()
                    .flatten()
                {
                    self.context_domain(&sub, own_context.clone(), context, domains, contexts)?;
                }
                Ok(())
            }
            Expr::Case(c) => {
                for branch in c.branches() {
                    self.context_domain(&branch, own_context.clone(), context, domains, contexts)?;
                }
                Ok(())
            }
            Expr::Restrict(r) => {
                if let Some(sub) = r.expr() {
                    let (_, extended) = self.restrict_domain(r, context)?;
                    self.context_domain(&sub, own_context, &extended, domains, contexts)?;
                }
                Ok(())
            }
            Expr::Dependence(d) => {
                let Some(f) = d.function() else { return Ok(()) };
                let maff = self.eval_function(&f, context)?;
                let processed = own_context
                    .apply(maff.into_map().map_err(|e| isl_err(e, d.syntax()))?)
                    .map_err(|e| isl_err(e, d.syntax()))?;
                if let Some(sub) = d.applied_expr() {
                    self.context_domain(&sub, processed, context, domains, contexts)?;
                }
                Ok(())
            }
            Expr::Select(s) => {
                let (rel, extended) = self.select_relation(s, context)?;
                let processed = own_context.apply(rel).map_err(|e| isl_err(e, s.syntax()))?;
                if let Some(sub) = s.expr() {
                    self.context_domain(&sub, processed, &extended, domains, contexts)?;
                }
                Ok(())
            }
            Expr::Reduce(r) => {
                let (proj, extended) = self.reduce_projection(r, context)?;
                let processed = own_context
                    .preimage_multi_aff(proj)
                    .map_err(|e| isl_err(e, r.syntax()))?;
                if let Some(body) = r.body() {
                    self.context_domain(&body, processed, &extended, domains, contexts)?;
                }
                Ok(())
            }
            Expr::Convolution(_) => {
                let (start, end) = range_of(expr.syntax());
                Err(Diagnostic::UnsupportedCalculatorOp {
                    operator: "convolution context-domain inference".to_string(),
                    start,
                    end,
                })
            }
            Expr::AutoRestrict(_) => unreachable!("handled above"),
        }
    }

    /// `AutoRestrictExpression`'s special context-domain rule (`inAutoRestrictExpression`): must
    /// be a direct child of a `CaseExpression`, at most one per case, and its inferred domain is
    /// the case's own context minus every *other* branch's expression domain (or, if it's the
    /// only branch, just the case's own context), intersected with its own expression domain.
    fn auto_restrict_context(
        &mut self,
        are: &ast::AutoRestrictExpr,
        parent_context: Set,
        context: &[String],
        domains: &Domains,
        contexts: &mut Domains,
    ) -> Result<(), Diagnostic> {
        let Some(parent) = are.syntax().parent() else {
            return Ok(());
        };
        let Some(case_expr) = ast::CaseExpr::cast(parent) else {
            let (start, end) = range_of(are.syntax());
            return Err(Diagnostic::AutoRestrictNotInCase { start, end });
        };
        let branches: Vec<Expr> = case_expr.branches().collect();
        let auto_restrict_count = branches
            .iter()
            .filter(|b| matches!(b, Expr::AutoRestrict(_)))
            .count();
        if auto_restrict_count > 1 {
            let (start, end) = range_of(are.syntax());
            return Err(Diagnostic::MultipleAutoRestrict { start, end });
        }
        let Some(own_domain) = domains.get(are.syntax()).cloned() else {
            return Ok(());
        };
        let others: Vec<&Expr> = branches
            .iter()
            .filter(|b| b.syntax() != are.syntax())
            .collect();
        let inferred = if others.is_empty() {
            parent_context
                .intersect(own_domain)
                .map_err(|e| isl_err(e, are.syntax()))?
        } else {
            let mut union: Option<Set> = None;
            for b in &others {
                let Some(d) = domains.get(b.syntax()) else {
                    return Ok(());
                };
                union = Some(match union {
                    None => d.clone(),
                    Some(u) => u.union(d.clone()).map_err(|e| isl_err(e, are.syntax()))?,
                });
            }
            parent_context
                .subtract(union.expect("others is non-empty"))
                .and_then(|s| s.intersect(own_domain))
                .map_err(|e| isl_err(e, are.syntax()))?
        };
        if inferred.is_empty().map_err(|e| isl_err(e, are.syntax()))? {
            let (start, end) = range_of(are.syntax());
            return Err(Diagnostic::EmptyAutoRestrict { start, end });
        }
        contexts.insert(are.syntax().clone(), inferred.clone());
        if let Some(sub) = are.expr() {
            self.context_domain(&sub, inferred, context, domains, contexts)?;
        }
        Ok(())
    }
}

fn require(expr: Option<Expr>, fallback: &SyntaxNode) -> Result<Expr, Diagnostic> {
    expr.ok_or_else(|| missing("sub-expression", fallback))
}

/// True for a `{: constraints}` domain with no explicit `[...]` tuple of its own — the parser
/// tags this `DOMAIN` when it appears as a `RestrictExpression`'s domain source (see
/// `alpha-syntax`'s `restrict_expr`) but `ARRAY_DOMAIN` everywhere else (`when`/`else` guards),
/// so callers here need to detect the bare-colon shape by content, not by node kind alone.
fn is_bare_colon_domain(text: &str) -> bool {
    text.trim_start_matches('{').trim_start().starts_with(':')
}
