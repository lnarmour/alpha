// Deliberately not "all of isl" — this is the bounded operation surface the core Alpha compiler
// actually needs: sets/maps, affine functions, constraints, piecewise-quasipolynomials
// (cardinality counting), and the AST builder (isl's loop-generation entry point).
#include <isl/ctx.h>
#include <isl/options.h>
#include <isl/id.h>
#include <isl/val.h>
#include <isl/space.h>
#include <isl/local_space.h>
#include <isl/set.h>
#include <isl/map.h>
#include <isl/aff.h>
#include <isl/constraint.h>
#include <isl/polynomial.h>
#include <isl/union_set.h>
#include <isl/union_map.h>
#include <isl/ast.h>
#include <isl/ast_build.h>
#include <isl/printer.h>
