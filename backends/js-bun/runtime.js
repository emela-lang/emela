function __emela_ok(value) {
  return { tag: 0, value };
}

function __emela_err(errorTag) {
  return { tag: 1, value: { tag: errorTag, value: undefined } };
}

function __emela_write_stdout_utf8(value) {
  try {
    if (typeof process !== "undefined" && process.stdout && process.stdout.write) {
      process.stdout.write(value);
    } else {
      console.log(value);
    }
    return __emela_ok(undefined);
  } catch (_) {
    return __emela_err(4);
  }
}

function __emela_read_stdin_utf8() {
  try {
    if (typeof Bun === "undefined" || !Bun.stdin) {
      return __emela_err(1);
    }
    return __emela_ok(Bun.stdin.text());
  } catch (_) {
    return __emela_err(4);
  }
}

function __emela_now_i32() {
  return Date.now() | 0;
}
