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

// The native addon links libisl/libgmp in statically (see editors/vscode/native's build, and
// isl-sys/build.rs's `ISL_STATIC` handling) specifically so it has no runtime dependency on them
// being installed on the system — this `require()` failing means something else is wrong (e.g. a
// corrupted install, or a platform this extension wasn't built for), not a missing library.
function loadNative(context: vscode.ExtensionContext, output: vscode.OutputChannel): NativeAddon | undefined {
  try {
    // eslint-disable-next-line @typescript-eslint/no-var-requires
    return require(path.join(context.extensionPath, 'native', 'index.js')) as NativeAddon;
  } catch (err) {
    output.appendLine(`Failed to load the native analyzer:\n${String(err)}`);
    void vscode.window
      .showErrorMessage(
        `Alpha: failed to load the native analyzer, so diagnostics are unavailable (syntax highlighting still works).`,
        'Show Log'
      )
      .then((choice) => {
        if (choice === 'Show Log') {
          output.show();
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
    vscode.workspace.onDidCloseTextDocument((document) => collection.delete(document.uri))
  );

  for (const document of vscode.workspace.textDocuments) {
    analyzeNow(document);
  }
}

export function deactivate(): void {}
