# Alpha Language (VS Code extension)

Syntax highlighting (a static TextMate grammar, `syntaxes/alpha.tmLanguage.json`) and live
diagnostics for `.alpha` files, backed by an in-process native addon (`native/`, napi-rs) that
calls straight into this workspace's `alpha-syntax`/`alpha-model` crates — no LSP server, no
external process. See the workspace root [`README.md`](../../README.md) for why.

The extension itself (`src/extension.ts`) does no analysis of its own: it debounces
edits/opens/saves, calls the native `checkAlphaSource(text)`, and turns the returned
diagnostics into `vscode.Diagnostic`s. Every actual check — parsing, name/domain resolution,
completeness, uniqueness, overlapping domains — lives in `alpha-model::check_source`
(`alpha-model/src/check.rs`), the one function the native binding (`native/src/lib.rs`) wraps.

## Installing

Not yet on the Marketplace (see below) — for now, grab the `.vsix` matching your platform from
the [Releases page](https://github.com/lnarmour/alpha/releases) (`alpha-language-darwin-arm64.vsix`,
`alpha-language-darwin-x64.vsix`, or `alpha-language-linux-x64.vsix`), then either run
`code --install-extension path/to/that.vsix`, or use the Extensions view's "..." menu → "Install
from VSIX…" inside VS Code. **Windows isn't supported** (matches the root README's existing
constraint — see "Requirements" below).

## Requirements

`libisl` and `libgmp` must be installed and discoverable on your system's shared-library search
path — same requirement as building `alphac` itself (see the root README), since the native
addon dynamically links them:

- **macOS**: `brew install isl gmp`
- **Linux**: install via your distro's package manager (e.g. on Debian/Ubuntu, `libisl` typically
  comes with whatever pulled it in as a dependency already; if not, `sudo apt-get install
  libisl23` or build isl from source)

If they're installed somewhere non-standard (a custom prefix, conda/nix environment, etc.) and
the extension can't find them, it'll show an error with a "Show Log"/"Open Settings" action
rather than failing silently — syntax highlighting keeps working regardless, only diagnostics
are affected. Point the extension at the right directory via the **`alpha.nativeLibraryPaths`**
setting (an array of directories, prepended to `DYLD_LIBRARY_PATH`/`LD_LIBRARY_PATH` before the
native module loads), then reload the window.

## Building from source

```
cd editors/vscode
npm install
npm run build:native   # builds native/ via @napi-rs/cli, produces native/index.js + the .node binary
npm run compile         # tsc: src/extension.ts -> out/extension.js
```

Also needs everything the root [`README.md`](../../README.md) lists to build `alpha-rs` itself
(Rust, isl + pkg-config, libclang), plus Node.js.

## Running in development

Open this directory (`editors/vscode`) in VS Code and press F5 (`Run Alpha extension`), or:

```
code --extensionDevelopmentPath="$(pwd)" /path/to/some/file.alpha
```

Open any `.alpha` file — syntax highlighting is immediate; diagnostics appear in the Problems
panel (and as squiggles) shortly after opening/editing/saving, sourced as `alphac`.

## Release process (for maintainers)

Versioning and releases for this extension are automated with
[release-please](https://github.com/googleapis/release-please), scoped to this directory
(`release-please-config.json`/`.release-please-manifest.json` at the repo root, component
`alpha-vscode`) — it reads [Conventional Commit](https://www.conventionalcommits.org/)-style PR
titles (already enforced repo-wide by the `pr-title` check in `.github/workflows/ci.yml`) to
maintain a standing "Release PR" with the next version bump and changelog. Merging that PR tags a
release (`alpha-vscode-vX.Y.Z`), which `.github/workflows/release-vscode.yml` then picks up to
build the native addon on macOS (x64 + arm64) and Linux (x64), package three platform-specific
`.vsix` files (`vsce package --target ...`), and attach them to the GitHub Release. **Marketplace
publishing is not part of this automated flow yet** — see below.

## Publishing to the Marketplace

Not done yet — nothing has been published. Steps for whoever sets this up:

1. Register a publisher at the
   [Marketplace publisher management page](https://marketplace.visualstudio.com/manage) (backed
   by a Microsoft/Azure DevOps account) — the publisher ID must then match the `"publisher"`
   field in `editors/vscode/package.json` (currently set to `lnarmour` as a placeholder; update
   it to match whatever ID actually gets registered).
2. Generate a Personal Access Token in Azure DevOps scoped to **Marketplace (Manage)**.
3. Locally: `npx @vscode/vsce login <publisher>` (prompts for the PAT), then, once a release's
   three `.vsix` files exist (from a GitHub Release, or built locally), publish all three
   platform variants under one version:
   ```
   npx @vscode/vsce publish --target darwin-x64 darwin-arm64 linux-x64
   ```
   VS Code (1.61+) automatically serves the right platform variant to each user.
4. Once that's working reliably, wiring an actual CI publish job is a small follow-up: add the
   PAT as a repo secret (e.g. `VSCE_PAT`) and add a step to `release-vscode.yml`'s `build` job
   running `vsce publish` — deliberately not done now, kept manual until there's a real publisher
   account to test against.

## Scope, for now

- Prebuilt for macOS (Intel + Apple Silicon) and Linux x86_64 (glibc) via CI. **No Windows
  support** — matches the root README's existing "Linux/macOS only" constraint (GMP/isl static
  linking complications on Windows).
- `isl`/`gmp` stay dynamically linked (not statically bundled) — see "Requirements" above for
  what that means for installing.
- All diagnostics are reported at `Error` severity, matching `alpha-model`'s own closed
  `Diagnostic` catalog (it already documents collapsing what the source Java system treated
  as warnings into hard errors, for consistency — no new severity tier invented here).
- If a file has syntax errors, only those are reported — semantic analysis (phases 1–6) only
  runs once the file parses cleanly, mirroring `alphac`'s own existing behavior.
