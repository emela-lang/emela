// The Emela VSCode client: starts `emela lsp` (spec 0033) over stdio and
// leaves everything else — diagnostics, completion — to the server.

import * as vscode from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;

export async function activate(context: vscode.ExtensionContext) {
  const configuration = vscode.workspace.getConfiguration("emela");
  const serverPath = configuration.get<string>("serverPath", "emela");
  const packageRoots = configuration.get<string[]>("packageRoots", []);
  const args = ["lsp", ...packageRoots.flatMap((root) => ["--package", root])];

  const serverOptions: ServerOptions = {
    command: serverPath,
    args,
  };
  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ scheme: "file", language: "emela" }],
  };

  client = new LanguageClient(
    "emela",
    "Emela Language Server",
    serverOptions,
    clientOptions,
  );
  await client.start();
  context.subscriptions.push({ dispose: () => client?.stop() });
}

export function deactivate(): Thenable<void> | undefined {
  return client?.stop();
}
