// Host functions for SystemFileSystem (file_system.emel) on JS. Uses `node:fs/promises`.
//
// The operations return an outcome object instead of throwing, since an external function cannot
// fail (spec 12.5): `{ error, message, text, names }`. `error` is "" on success, or the name of the
// Emela error to fail with. Each operation maps the host's error codes to the errors its `fails`
// lists, and anything else to "FileSystemFailure" with the host's message.

import * as fs from "node:fs/promises";

const ok = (fields) => ({ error: "", message: "", text: "", names: [], ...fields });
const failed = (error, message) => ({ error, message, text: "", names: [] });

// Codes that mean the path or one of its parents does not exist. ENOTDIR (a file in the middle
// of the path) is the same as not existing, except where an operation reports NotADirectory.
const MISSING = new Set(["ENOENT", "ENOTDIR"]);
const DENIED = new Set(["EACCES", "EPERM"]);

// Turns a host exception into an outcome. `codes` maps extra error codes to error names.
const classify = (e, codes) => {
  const code = e?.code;
  if (code !== undefined && Object.hasOwn(codes, code)) return failed(codes[code], "");
  if (MISSING.has(code)) return failed("FileNotFound", "");
  if (DENIED.has(code)) return failed("FileAccessDenied", "");
  return failed("FileSystemFailure", e instanceof Error ? e.message : String(e));
};

// `fatal` rejects invalid UTF-8 instead of replacing it. `ignoreBOM` keeps a leading BOM in the
// text, so that writing the text back does not change the file.
const decoder = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });

export async function readText(path) {
  let bytes;
  try {
    bytes = await fs.readFile(path);
  } catch (e) {
    return classify(e, { EISDIR: "IsADirectory" });
  }
  try {
    return ok({ text: decoder.decode(bytes) });
  } catch {
    return failed("InvalidText", "");
  }
}

export async function writeText(path, text) {
  try {
    await fs.writeFile(path, text, "utf8");
    return ok({});
  } catch (e) {
    return classify(e, { EISDIR: "IsADirectory" });
  }
}

export async function appendText(path, text) {
  try {
    await fs.appendFile(path, text, "utf8");
    return ok({});
  } catch (e) {
    return classify(e, { EISDIR: "IsADirectory" });
  }
}

// False on any error, including a permission error.
export async function exists(path) {
  try {
    await fs.stat(path);
    return true;
  } catch {
    return false;
  }
}

export async function isDirectory(path) {
  try {
    return (await fs.stat(path)).isDirectory();
  } catch {
    return false;
  }
}

// Compares by code points (spec 10.6), unlike the default sort, which compares UTF-16 code units.
const byCodePoints = (a, b) => {
  const xs = [...a];
  const ys = [...b];
  for (let i = 0; i < Math.min(xs.length, ys.length); i++) {
    const d = xs[i].codePointAt(0) - ys[i].codePointAt(0);
    if (d !== 0) return d;
  }
  return xs.length - ys.length;
};

export async function listDirectory(path) {
  try {
    const names = await fs.readdir(path);
    return ok({ names: names.sort(byCodePoints) });
  } catch (e) {
    // A file at `path` itself is ENOTDIR; a file in the middle of `path` is too.
    return classify(e, { ENOTDIR: "NotADirectory" });
  }
}

// Creates the parents too and does nothing if the directory exists. A file at `path` or in the
// middle of it is NotADirectory.
export async function createDirectory(path) {
  try {
    await fs.mkdir(path, { recursive: true });
  } catch (e) {
    return classify(e, { ENOTDIR: "NotADirectory", EEXIST: "NotADirectory" });
  }
  // `mkdir` with `recursive` succeeds silently on some hosts when `path` is an existing file.
  if (!(await isDirectory(path))) return failed("NotADirectory", "");
  return ok({});
}

// Removes a file or an empty directory. A symbolic link is removed itself, not its target.
export async function remove(path) {
  try {
    const stat = await fs.lstat(path);
    if (stat.isDirectory()) await fs.rmdir(path);
    else await fs.unlink(path);
    return ok({});
  } catch (e) {
    return classify(e, { ENOTEMPTY: "DirectoryNotEmpty", EEXIST: "DirectoryNotEmpty" });
  }
}

export const error = (outcome) => outcome.error;
export const message = (outcome) => outcome.message;
export const text = (outcome) => outcome.text;
export const nameCount = (outcome) => outcome.names.length;
export const nameAt = (outcome, i) => outcome.names[i];
// Only C frees outcomes (file_system.c); the garbage collector does it on JS.
export const release = (_outcome) => {};
