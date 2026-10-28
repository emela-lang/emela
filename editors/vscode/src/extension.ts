// The Emela VSCode client: starts `emela lsp` over stdio and leaves the language features to the
// server. The client itself only finds the binary, restarts the server, and shows its state in
// the status bar.

import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  State,
} from "vscode-languageclient/node";

const INSTALL_URL = "https://github.com/emela-lang/emela#install";

let client: LanguageClient | undefined;
let outputChannel: vscode.OutputChannel;
let statusBarItem: vscode.StatusBarItem;
// Starts and stops run one after another, so that two quick restarts do not leave two servers.
let lifecycle: Promise<void> = Promise.resolve();

export async function activate(context: vscode.ExtensionContext) {
  // One channel for every server started in this session, so a restart does not open a new one.
  outputChannel = vscode.window.createOutputChannel("Emela Language Server");
  statusBarItem = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 0);
  statusBarItem.name = "Emela Language Server";
  statusBarItem.command = "emela.restartServer";
  context.subscriptions.push(
    outputChannel,
    statusBarItem,
    vscode.commands.registerCommand("emela.restartServer", restartClient),
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (event.affectsConfiguration("emela.serverPath")) {
        void restartClient();
      }
    }),
    vscode.window.onDidChangeActiveTextEditor(updateStatusBarVisibility),
  );
  updateStatusBarVisibility(vscode.window.activeTextEditor);
  await enqueue(startClient);
}

export function deactivate(): Thenable<void> | undefined {
  return enqueue(stopClient);
}

function enqueue(step: () => Promise<void>): Promise<void> {
  lifecycle = lifecycle.then(step, step);
  return lifecycle;
}

function restartClient(): Promise<void> {
  return enqueue(async () => {
    await stopClient();
    await startClient();
  });
}

async function startClient(): Promise<void> {
  const configured = vscode.workspace.getConfiguration("emela").get<string>("serverPath", "emela");
  const command = findServer(configured);
  if (command === undefined) {
    setStatus("error", `\`${configured}\` was not found`);
    void showServerNotFound(configured);
    return;
  }

  const serverOptions: ServerOptions = { command, args: ["lsp"] };
  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ scheme: "file", language: "emela" }],
    outputChannel,
  };
  const newClient = new LanguageClient(
    "emela",
    "Emela Language Server",
    serverOptions,
    clientOptions,
  );
  newClient.onDidChangeState((event) => {
    if (client !== newClient) {
      return;
    }
    switch (event.newState) {
      case State.Starting:
        setStatus("starting", command);
        break;
      case State.Running:
        setStatus("running", command);
        break;
      case State.Stopped:
        setStatus("stopped", command);
        break;
    }
  });
  client = newClient;
  setStatus("starting", command);
  try {
    await newClient.start();
  } catch (error) {
    // The client has already reported the failure in a notification and in the output channel.
    setStatus("error", `${command}: ${String(error)}`);
  }
}

async function stopClient(): Promise<void> {
  const oldClient = client;
  client = undefined;
  if (oldClient === undefined) {
    return;
  }
  try {
    await oldClient.stop();
  } catch (error) {
    // A server that never started, or already exited, cannot be stopped; dispose of it anyway.
    outputChannel.appendLine(`Failed to stop the language server: ${String(error)}`);
  }
}

/**
 * Resolves the `emela.serverPath` setting to an executable file, or `undefined` if there is none.
 *
 * A value with a directory part is used as written (after expanding a leading `~`). A bare name is
 * looked up on `PATH`, and then in `~/.emela/bin`, where `install.sh` puts the binary by default:
 * an editor started from the desktop often does not see the `PATH` of the login shell.
 */
function findServer(serverPath: string): string | undefined {
  const expanded = serverPath.replace(/^~(?=$|[\\/])/, os.homedir());
  if (expanded.includes("/") || expanded.includes("\\")) {
    return isExecutable(expanded) ? expanded : undefined;
  }
  const directories = (process.env.PATH ?? "").split(path.delimiter).filter((d) => d !== "");
  directories.push(path.join(os.homedir(), ".emela", "bin"));
  const extensions =
    process.platform === "win32" ? (process.env.PATHEXT ?? ".EXE").split(";").concat("") : [""];
  for (const directory of directories) {
    for (const extension of extensions) {
      const candidate = path.join(directory, expanded + extension);
      if (isExecutable(candidate)) {
        return candidate;
      }
    }
  }
  return undefined;
}

function isExecutable(file: string): boolean {
  try {
    if (!fs.statSync(file).isFile()) {
      return false;
    }
    fs.accessSync(file, fs.constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

async function showServerNotFound(serverPath: string): Promise<void> {
  const choice = await vscode.window.showErrorMessage(
    `Emela: the \`emela\` binary was not found (emela.serverPath is \`${serverPath}\`). ` +
      "Install Emela, or set emela.serverPath to the absolute path of the binary.",
    "How to Install",
    "Open Settings",
    "Retry",
  );
  switch (choice) {
    case "How to Install":
      await vscode.env.openExternal(vscode.Uri.parse(INSTALL_URL));
      break;
    case "Open Settings":
      await vscode.commands.executeCommand("workbench.action.openSettings", "emela.serverPath");
      break;
    case "Retry":
      await restartClient();
      break;
  }
}

type Status = "starting" | "running" | "stopped" | "error";

function setStatus(status: Status, detail: string): void {
  const icon = {
    starting: "$(sync~spin)",
    running: "$(check)",
    stopped: "$(debug-stop)",
    error: "$(error)",
  }[status];
  statusBarItem.text = `${icon} Emela`;
  statusBarItem.tooltip = `Emela Language Server: ${status}\n${detail}\n\nClick to restart.`;
  statusBarItem.backgroundColor =
    status === "error" ? new vscode.ThemeColor("statusBarItem.errorBackground") : undefined;
}

// The item is shown only while an Emela file is in front, as with other language status items.
function updateStatusBarVisibility(editor: vscode.TextEditor | undefined): void {
  if (editor?.document.languageId === "emela") {
    statusBarItem.show();
  } else {
    statusBarItem.hide();
  }
}
