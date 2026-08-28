# Alphalang Linear Types Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Expose immutable linear-variable metadata in Python, enforce root-aware signatures, and add an executable notebook explaining linear resource validity and schedule legality.

**Architecture:** Keep compiler semantics in Rust and expose snapshots of the multiplicity metadata already carried by transform IR. Make binding parsing consult `analyze_root` before recreating the selected system resolver for lowering. Teach the behavior through one concise, executed `nbval` notebook.

**Tech Stack:** Rust, PyO3, Python 3.9+, pytest, Jupyter, nbval, maturin/uv.

**Spec:** `docs/superpowers/specs/2026-08-28-alphalang-linear-types-design.md`

## Global Constraints

- Preserve the immutable `System -> NormalizedSystem -> ScheduledSystem` pipeline.
- Keep `ValueError` for syntax/semantic failures and `ScheduleError` for schedule failures.
- Preserve `parse()` selecting the first system in a source root.
- Do not expose mutable compiler IR or add structured diagnostic classes.
- Do not stage or alter the unrelated `Cargo.lock` change.
- Notebook cells must have valid JSON and `metadata.language`; existing cells retain their IDs.

---

### Task 1: Root-Aware Python Analysis

**Files:**
- Modify: `alphalang/tests/test_alpha.py`
- Modify: `alphalang/src/lib.rs`

**Interfaces:**
- Consumes: `alpha_model::analyze_root(ctx, root)` and `Resolver::analyze_system`.
- Produces: unchanged `alphalang.parse(source) -> System` with root/package signature enforcement.

- [x] **Step 1: Add failing Python tests** proving `external move(linear) -> linear` succeeds and `external f(1)` rejects a linear argument.
- [x] **Step 2: Reinstall the extension and run the two tests**, confirming the explicit signature test fails under system-local analysis.
- [x] **Step 3: Change `parse_and_lower`** to run `analyze_root`, report first-system/whole-program diagnostics, then initialize a fresh resolver with `resolver.analyze_system(&system)` before lowering.
- [x] **Step 4: Reinstall and rerun the focused tests**, then run all `alphalang/tests/test_alpha.py` tests.
- [x] **Step 5: Commit** with `git commit -m "fix: enforce root signatures in alphalang"`.

### Task 2: Immutable Python Multiplicity Metadata

**Files:**
- Modify: `alphalang/tests/test_alpha.py`
- Modify: `alphalang/src/lib.rs`
- Modify: `alphalang/python/alphalang/__init__.py`

**Interfaces:**
- Produces: `Multiplicity.LINEAR`, `Multiplicity.UNRESTRICTED`, `Variable.name`, `Variable.domain`, `Variable.multiplicity`, and tuple properties `inputs`, `outputs`, `locals` on all pipeline stages.

- [x] **Step 1: Add failing Python tests** for variable metadata, unrestricted metadata, and preservation through normalization/scheduling.
- [x] **Step 2: Reinstall and run the focused tests**, confirming the classes/properties are absent.
- [x] **Step 3: Implement frozen PyO3 `Multiplicity` and `Variable` snapshots**, conversion from transform IR, stable `repr`/equality, and interface properties shared across the three pipeline wrappers.
- [x] **Step 4: Re-export both types** from `alphalang/__init__.py` and update `__all__`.
- [x] **Step 5: Reinstall and run all Python binding and magic tests**.
- [x] **Step 6: Commit** with `git commit -m "feat: expose linear metadata in alphalang"`.

### Task 3: Linear Types And Scheduling Notebook

**Files:**
- Create: `alphalang/notebooks/linear_types.ipynb`
- Modify: `alphalang/notebooks/README.md`
- Modify: `alphalang/README.md`

**Interfaces:**
- Consumes: metadata API, `normalize`, `schedule`, `generate`, `ValueError`, and `ScheduleError`.
- Produces: an executed `nbval` tutorial and documented test commands.

- [x] **Step 1: Create the notebook JSON** with markdown and executable Python cells covering metadata, exact-once failures, explicit external signatures, C generation, two legal transfer schedules, a legal producer-consumer schedule, and an illegal reversed dependence.
- [x] **Step 2: Execute the notebook in place** with `uv run jupyter nbconvert --to notebook --execute --inplace alphalang/notebooks/linear_types.ipynb` so saved output is real.
- [x] **Step 3: Run `uv run pytest --nbval alphalang/notebooks/linear_types.ipynb`** and repair only notebook/API mismatches until it passes.
- [x] **Step 4: Update both README files** to document metadata, the notebook, legal schedule properties, and notebook test commands.
- [x] **Step 5: Run final validation**: `uv run pytest alphalang/tests`, both notebook `nbval` tests, `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace --no-fail-fast`.
- [x] **Step 6: Confirm `git diff --check` and `git status --short`**, leaving `Cargo.lock` unstaged.
- [x] **Step 7: Commit** with `git commit -m "docs: add linear types notebook"`.