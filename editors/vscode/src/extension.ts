import * as vscode from 'vscode';
import * as path from 'path';

// One diagnostic from the Rust side (`editors/vscode/native`, which is a thin shim over
// `alpha_model::check_source` — all analysis lives there, not in this file).
interface JsDiagnostic {
  message: string;
  start: number;
  end: number;
  system?: string;
}

interface NativeAddon {
  checkAlphaSource(source: string): JsDiagnostic[];
}

const DEBOUNCE_MS = 300;
const CONFIG_SECTION = 'alpha';
const LIBRARY_PATH_SETTING = 'nativeLibraryPaths';

// `start`/`end` are UTF-8 byte offsets (matching `alpha_syntax::SyntaxError`/`Diagnostic::range`),
// but `vscode.Position`/`TextDocument.positionAt` expect UTF-16 code-unit offsets. Convert via the
// document's own bytes rather than assuming ASCII-only source.
function positionAtByteOffset(document: vscode.TextDocument, byteOffset: number): vscode.Position {
  const bytes = Buffer.from(document.getText(), 'utf8');
  const prefix = bytes.subarray(0, Math.min(byteOffset, bytes.length));
  return document.positionAt(prefix.toString('utf8').length);
}

function toDiagnostic(document: vscode.TextDocument, d: JsDiagnostic): vscode.Diagnostic {
  const range = new vscode.Range(
    positionAtByteOffset(document, d.start),
    positionAtByteOffset(document, d.end)
  );
  const message = d.system ? `[${d.system}] ${d.message}` : d.message;
  const diagnostic = new vscode.Diagnostic(range, message, vscode.DiagnosticSeverity.Error);
  diagnostic.source = 'alphac';
  return diagnostic;
}

// `isl`/`gmp` are dynamically linked (see editors/vscode/README.md) — on a machine where they
// aren't on the default system library search path, the native addon's own `require()` below
// would otherwise throw a raw dlopen error. Let the `alpha.nativeLibraryPaths` setting add extra
// search directories, the same way a user would set DYLD_LIBRARY_PATH/LD_LIBRARY_PATH by hand.
// Must run before the native module's first `require()` — dlopen happens at that point.
function applyNativeLibraryPathSetting(): void {
  const extraPaths = vscode.workspace
    .getConfiguration(CONFIG_SECTION)
    .get<string[]>(LIBRARY_PATH_SETTING, []);
  if (extraPaths.length === 0) {
    return;
  }
  const varName = process.platform === 'darwin' ? 'DYLD_LIBRARY_PATH' : 'LD_LIBRARY_PATH';
  const existing = process.env[varName];
  process.env[varName] = existing ? `${extraPaths.join(':')}:${existing}` : extraPaths.join(':');
}

function loadNative(context: vscode.ExtensionContext, output: vscode.OutputChannel): NativeAddon | undefined {
  applyNativeLibraryPathSetting();
  try {
    // eslint-disable-next-line @typescript-eslint/no-var-requires
    return require(path.join(context.extensionPath, 'native', 'index.js')) as NativeAddon;
  } catch (err) {
    output.appendLine(`Failed to load the native analyzer:\n${String(err)}`);
    const installHint =
      process.platform === 'darwin' ? 'brew install isl gmp' : "your package manager's isl/gmp packages (e.g. apt)";
    void vscode.window
      .showErrorMessage(
        `Alpha: failed to load the native analyzer, so diagnostics are unavailable (syntax highlighting still works). ` +
          `This usually means libisl/libgmp aren't installed or aren't on the library search path — try ${installHint}, ` +
          `or set the "alpha.nativeLibraryPaths" setting to point at a custom install location.`,
        'Show Log',
        'Open Settings'
      )
      .then((choice) => {
        if (choice === 'Show Log') {
          output.show();
        } else if (choice === 'Open Settings') {
          void vscode.commands.executeCommand(
            'workbench.action.openSettings',
            `${CONFIG_SECTION}.${LIBRARY_PATH_SETTING}`
          );
        }
      });
    return undefined;
  }
}

export function activate(context: vscode.ExtensionContext): void {
  const output = vscode.window.createOutputChannel('Alpha');
  context.subscriptions.push(output);

  const native = loadNative(context, output);

  const collection = vscode.languages.createDiagnosticCollection('alpha');
  context.subscriptions.push(collection);

  const pending = new Map<string, ReturnType<typeof setTimeout>>();

  function analyzeNow(document: vscode.TextDocument): void {
    if (document.languageId !== 'alpha' || !native) {
      return;
    }
    let results: JsDiagnostic[];
    try {
      results = native.checkAlphaSource(document.getText());
    } catch (err) {
      collection.set(document.uri, [
        new vscode.Diagnostic(
          new vscode.Range(0, 0, 0, 0),
          `alphac: internal error analyzing this file: ${String(err)}`,
          vscode.DiagnosticSeverity.Error
        ),
      ]);
      return;
    }
    collection.set(document.uri, results.map((d) => toDiagnostic(document, d)));
  }

  function scheduleAnalysis(document: vscode.TextDocument): void {
    if (document.languageId !== 'alpha') {
      return;
    }
    const key = document.uri.toString();
    const existing = pending.get(key);
    if (existing) {
      clearTimeout(existing);
    }
    pending.set(
      key,
      setTimeout(() => {
        pending.delete(key);
        analyzeNow(document);
      }, DEBOUNCE_MS)
    );
  }

  context.subscriptions.push(
    vscode.workspace.onDidOpenTextDocument(analyzeNow),
    vscode.workspace.onDidChangeTextDocument((e) => scheduleAnalysis(e.document)),
    vscode.workspace.onDidSaveTextDocument(analyzeNow),
    vscode.workspace.onDidCloseTextDocument((document) => collection.delete(document.uri)),
    vscode.workspace.onDidChangeConfiguration((e) => {
      if (e.affectsConfiguration(`${CONFIG_SECTION}.${LIBRARY_PATH_SETTING}`)) {
        void vscode.window
          .showInformationMessage('Alpha: reload the window for the updated library path to take effect.', 'Reload Window')
          .then((choice) => {
            if (choice === 'Reload Window') {
              void vscode.commands.executeCommand('workbench.action.reloadWindow');
            }
          });
      }
    })
  );

  for (const document of vscode.workspace.textDocuments) {
    analyzeNow(document);
  }
}

export function deactivate(): void {}
