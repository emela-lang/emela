// The Emela test runner (spec chapter 13). Embedded at the top of the test entry module.
//
// Calls the test functions one by one and writes one result per line to stdout. Each line is `\x1eemela-test ` followed by JSON.
// Displaying the results (`test name ... ok`) is up to the caller (emela-driver).
// When inlined, the leading `export ` of each line is stripped, so write `export` only at the start of a line.

const $TEST_EVENT = "\x1eemela-test ";

function $testReport(event) {
  process.stdout.write($TEST_EVENT + JSON.stringify(event) + "\n");
}

// If the value returned by a test function is an unhandled error (13.3), returns its display.
// A failure comes back as the runtime's `$Fail` (16.5). The $Fail class is not loaded here, so it is recognized by a `Symbol.for` key.
function $testUnhandledError(value) {
  const show = value?.[Symbol.for("emela.fail")];
  return typeof show === "function" ? show.call(value) : undefined;
}

// Displays an exception thrown by a test function. A defect is an Error whose `name` is "Defect" (7.5).
// Other exceptions (thrown by external functions) are treated as defects too, prefixed with their name.
function $testDescribe(error) {
  if (error instanceof Error) {
    return error.name === "Defect" ? error.message : `${error.name}: ${error.message}`;
  }
  return String(error);
}

// Ties tests left suspended without resume (14.5), and exceptions thrown inside host callbacks,
// to the running test. `$testCurrent` is the running test's `{ defect, interrupt }`.
let $testCurrent = null;

// If the event loop is empty but the running test has not finished, that test is left suspended.
process.on("beforeExit", () => {
  $testCurrent?.interrupt();
});

process.on("uncaughtException", (error) => {
  if ($testCurrent !== null) {
    $testCurrent.defect(error);
  } else {
    process.stderr.write(`${error?.stack ?? String(error)}\n`, () => process.exit(1));
  }
});

function $testIsGenerator(value) {
  return Object.prototype.toString.call(value) === "[object Generator]";
}

// Calls one test function and returns its result. A suspending test function (one that returns a generator) is driven by `start` (the runtime's
// `$start`), waiting until it finishes, hits a defect, or unwinds while left suspended.
// The test function is the root fiber of its run: only how it ends is reported. An interruption of a child fiber
// (11.4) ends that child as an Interrupt, which its scope receives (11.5), so it never reaches here.
function $runTest(test, start) {
  return new Promise((resolve) => {
    let over = false;
    let stop;
    const finish = (result) => {
      if (over) return;
      over = true;
      $testCurrent = null;
      stop?.();
      resolve(result);
    };
    const value = (v) => {
      const error = $testUnhandledError(v);
      finish(error === undefined ? { outcome: "ok" } : { outcome: "error", message: error });
    };
    // A failed assert's defect has its location (13.2), shown apart from the message.
    const defect = (error) =>
      finish(
        error instanceof Error && error.name === "Interrupted"
          ? { outcome: "interrupted" }
          : error instanceof Error && error.name === "Defect" && typeof error.location === "string"
            ? { outcome: "defect", message: $testDescribe(error), location: error.location }
            : { outcome: "defect", message: $testDescribe(error) },
      );
    // Left suspended: unwind it through `stop`, which runs the releases of its open withs (14.5) and then
    // reports `$Interrupted` (or a defect a release raised) to `defect`.
    const interrupt = () => (stop === undefined ? finish({ outcome: "interrupted" }) : stop());
    $testCurrent = { defect, interrupt };
    let v;
    try {
      v = test();
    } catch (error) {
      defect(error);
      return;
    }
    if (typeof start === "function" && $testIsGenerator(v)) {
      stop = start(v, (ok, result) => (ok ? value(result) : defect(result)));
    } else {
      value(v);
    }
  });
}

// `tests` is an array of [display name, function]. A failure in one does not stop the rest (13.3).
// `start` is the runtime's `$start` exported by the output module (undefined if no test function can suspend).
export async function $runTests(tests, start) {
  for (const [name, test] of tests) {
    const result = await $runTest(test, start);
    $testReport({ event: "result", name, ...result });
  }
  $testReport({ event: "done" });
}
