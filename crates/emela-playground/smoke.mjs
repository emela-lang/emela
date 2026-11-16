// Loads the built compiler for the browser (scripts/build-playground.sh), compiles a program and
// runs its output without `process`, as a browser Worker would: the runtime then writes through
// `console.log`, and `with Platform` loads although there is no file system.
//
// Usage: node crates/emela-playground/smoke.mjs [DIR]   (default: target/playground)

import { readFileSync, mkdtempSync, mkdirSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import assert from 'node:assert/strict';

const dir = resolve(process.argv[2] ?? 'target/playground');
const { initSync, check, format, compile } = await import(pathToFileURL(join(dir, 'emela_playground.js')).href);
initSync({ module: readFileSync(join(dir, 'emela_playground_bg.wasm')) });

const source = `import Std.Console.{Console}
import Std.FileSystem.{FileSystem}
import Std.Platform.{Platform}

error NotFound(id: Int)

fn find(id: Int) -> String fails NotFound {
  if id == 1 { "alice" } else { fail NotFound(id:) }
}

fn main() {
  with Platform {
    console = use Console
    console.print_line("Hello, #{find(1)}!")
    console.print_line(find(2) escape { NotFound(_) -> "guest" })
    [1, 2, 3].each(|x| console.print_line("#{x * 2}"))
    console.print_line("#{(use FileSystem).exists("main.emel")}")
  }
}
`;

assert.equal(JSON.parse(check(source)).ok, true);
assert.equal(JSON.parse(check('fn main() { x }')).ok, false);
assert.equal(JSON.parse(format('fn main( ){dbg( 1 )}')).source, 'fn main() { dbg(1) }\n');

const compiled = JSON.parse(compile(source));
assert.equal(compiled.ok, true, compiled.diagnostics);
const out = mkdtempSync(join(tmpdir(), 'emela-playground-'));
for (const file of compiled.files) {
  mkdirSync(dirname(join(out, file.path)), { recursive: true });
  writeFileSync(join(out, file.path), file.text);
}

// Hide `process` the way a browser does not have it, and catch what the runtime writes.
const lines = [];
const log = console.log;
const hidden = globalThis.process;
console.log = (text) => lines.push(String(text));
Object.defineProperty(globalThis, 'process', { value: undefined, configurable: true });
try {
  const entry = await import(pathToFileURL(join(out, compiled.entry)).href);
  // A main that may suspend returns a generator, which the runtime's `$start` drives.
  await new Promise((resolve, reject) => {
    const value = entry.main();
    if (Object.prototype.toString.call(value) === '[object Generator]') {
      entry.$start(value, (ok, result) => (ok ? resolve(result) : reject(result)));
    } else {
      resolve(value);
    }
  });
} finally {
  Object.defineProperty(globalThis, 'process', { value: hidden, configurable: true });
  console.log = log;
}
assert.deepEqual(lines, ['Hello, alice!', 'guest', '2', '4', '6', 'False']);
console.log('playground smoke test passed');
