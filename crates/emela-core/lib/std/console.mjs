// Host functions for SystemConsole (console.emel) on JS.
//
// The runtime writes, through the same path as SystemStdout, SystemStderr and `dbg`, so that writes mixed between
// them keep their order (`[api.std.process]`). On a host without a standard output it writes to `console.log` and
// `console.error`. The runtime registers it under this key when it is loaded, before any operation runs.

const OUTPUT = Symbol.for("emela.output");

// Without the runtime (an output that calls nothing of it), the host's console.
function write(fd, text) {
  const output = globalThis[OUTPUT];
  if (output !== undefined) output.write(fd, text);
  else if (fd === 2) console.error(text.endsWith("\n") ? text.slice(0, -1) : text);
  else console.log(text.endsWith("\n") ? text.slice(0, -1) : text);
}

export const printLine = (text) => write(1, `${text}\n`);

export const printErrorLine = (text) => write(2, `${text}\n`);
