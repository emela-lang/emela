---
title: A sampling profiler for emela build, run and test
authors: [Alliana]
status: draft
issue:
touches: [cmd.profile (new), cmd.build, cmd.test.target, target.native.output, target.native.cc, target.native.run, diag.list]
requires: []
related: []
---

# A sampling profiler for emela build, run and test

## Summary

`emela run --profile` and `emela test --profile` run the program under a sampling profiler of the operating system and report where its time goes, by Emela function. `emela build --profile` makes the same build without running it, with symbols named after Emela functions, for any profiler the user runs. The first version supports only the native target. The profile is taken from the same optimised code that `emela run` runs, so it answers both "which of my functions is slow" and "what did the compiler make of them".

## Motivation

- There is no way to see where an Emela program spends its time. The profilers of the operating system can run `target/emela/native/main`, but what they show cannot be read back to Emela source:
  - The emitted C is compiled without debug information (`-O2`, no `-g`), so the profiler cannot tell which functions a C function was inlined into.
  - A function appears under the name of its C symbol (`e2_fib` for `fib`), not under its Emela path.
- Two uses need the same answer:
  - Making a program faster: the functions that take the most time, in the program as it is built.
  - Checking the output of the compiler: which functions remain as frames after inlining and closure devirtualisation (`docs/optimization.md`), and how much time each takes. Instrumenting the code would change the code being measured, so this needs sampling of the optimised code.
- A workaround exists only for someone who knows the emitted C: set `EMELA_CC_ARGS=-g`, run the profiler by hand, and translate the symbols. The names of the symbols are not specified, and they change from build to build of the compiler.

## Proposal

### Syntax

A new flag for `emela build`, `emela run` and `emela test`:

```
emela build --profile [path]
emela run --profile [path] [-- args...]
emela test --profile [path] [filter]
```

`emela check` does not take it. `emela build --profile` makes the profiling build without running it, for a profiler that the user starts.

### Meaning

Building:

- With `--profile`, the program is built for the build target as without it, at the same optimisation level, with these additions:
  - The C compiler is also given the options that produce debug information and keep frame pointers.
  - The symbol of each function of the executable that comes from an Emela function starts with the path of that function in the program (`Main.fib`). What follows the path (how copies made by monomorphisation are told apart, for example) is unspecified. A profiler, a debugger or a backtrace started by the user shows these names without help from the command.
- To name the symbols, the emitted C of a profiling build uses assembler labels, an extension of C that GCC and Clang accept. This is an exception to "the emitted C compiles as C11" (`[target.native.cc]`): a profiling build needs a C compiler that accepts assembler labels, and with one that does not, it fails with E9103. A build without `--profile` stays within C11.
- The executable does not change in meaning. Its output and exit code are as without `--profile` (`[cmd.build]`, `[cmd.test]`).
- The output directory is the same as without `--profile`. A later build without `--profile` replaces the executable.
- `emela build --profile` stops here. `emela run --profile` and `emela test --profile` go on to run the program as follows.

Running:

- The command starts the program under the profiler of the operating system, which samples the call stack at a fixed interval. The interval is chosen by the implementation, and is not set by a flag.
- When the program ends, the command writes the profile to the output directory and prints a summary to standard error. It does so however the program ends (with a value, an error, a defect or an interruption), and the exit code is the exit code of the program (`[cmd.build]`, `[cmd.test.exit]`).
- When the profiler cannot be started or fails, the program's result is not reported as a profile: it is E9101 or E9104, and the exit code is 1.
- A frame of the profile is an Emela function, named by its path in the program (`Main.fib`) and located at its definition. The profile does not resolve lines inside a function; an implementation MAY add them.
- A function that the compiler inlined into another appears in one of two ways:
  - as a frame of its own, marked as inlined, when the debug information of the executable records the inlining;
  - otherwise, counted in the function it was inlined into. Which inlining is shown as its own frame is implementation-defined.
- The summary lists the functions with the most samples in which they are the innermost Emela frame (self time), with the number of samples, their share of all samples, the location of the function, and whether it was inlined. Functions are named as in the profile, not by a symbol of the emitted C.
- The summary shows the total number of samples. When there are too few for the shares to mean much, it says so in one line; the threshold is chosen by the implementation.
- Frames of the runtime and of the C library are counted, and each is named as one group (`<runtime>`, `<libc>`), not by its symbol.
- The command MAY write other files next to the profile file, to view the profile (a page for a browser, for example).
- The name and format of the profile file, of any other file it writes, and the layout of the summary are unspecified, and an implementation MAY change them in any release. Only the presence of the file and the meaning of the numbers in the summary are specified.
- `--profile` is an ordinary flag, not an experimental feature (`[intro.experimental]`): it does not change the meaning of the program, and it is not read from `Pome.toml`. What is committed is the name of the flag and that the program behaves as without it.

`emela test --profile`:

- The whole test process is profiled as one run (`[target.native.run]` runs every test in one process).
- The summary additionally lists the elapsed time of each test, longest first. The `plain` reporter (`[cmd.test.output]`) is unchanged: the times are in the summary on standard error, not on standard output.

Targets:

- `--profile` is supported for `--target native` only. With another target it is an error in the command-line arguments, and nothing is built.
- It works on the platforms that the native target supports (`[target]`). The profiler is Time Profiler (`xctrace`) on macOS and `perf` on Linux. A platform that the native target adds later names its profiler in the proposal that adds it.

### Typing rules

None.

## Diagnostics

- [ ] E9101 also covers a profiler that is not found or cannot be started (`` cannot start `perf`: <reason> ``).
- [ ] E9104 (new): the profiler started but failed, or the operating system did not allow it to sample the program. The standard error of the profiler is attached as a note, cut off as in E9103. On Linux, when sampling is not permitted, the help names `kernel.perf_event_paranoid`.
- [ ] `--profile` with a target other than `native`: an error in the command-line arguments, as for an unknown value of `--target`.

## Compatibility

- Programs and their meaning do not change. Builds without `--profile` do not change.
- The profile file and the summary are outside the compatibility guarantees.

## Implementation

- `emela-codegen-c`: in a profiling build, give each emitted C function an assembler label with the Emela path of the function it comes from (`static int32_t e2_fib(int32_t v0) __asm__("Main.fib");`), with a suffix for copies made by monomorphisation. A prototype with Clang at `-std=c11 -O2` gives the symbol `Main.fib` in `nm`. Also write, next to the executable, the location of the definition of each function, for the frames of the profile. Core IR, LIR and the C backend carry no source locations, so locations are per function.
- Check which characters the profilers and `c++filt`-style demanglers accept in symbols (`.`, `[`, `,`) before fixing the form of the suffix.
- `emela-driver` (`c.rs`): add `-g -fno-omit-frame-pointer` to `CC_FLAGS` when profiling.
- Driver, running:
  - macOS: `xctrace record --template 'Time Profiler' --launch -- main ...`, then `xctrace export` to read the samples.
  - Linux: `perf record -g -- main ...`, then `perf script` to read the samples.
  - Fold the stacks, map symbols to Emela paths, group runtime and libc frames, write the profile file, print the summary.
  - The profile file is `profile.folded` in the output directory, in folded stacks: one line per distinct stack, the frames from the outermost joined by `;`, a space, and the number of samples. A frame is the Emela path of the function and the location of its definition (`Main.fib@src/main.emel:3`). An inlined frame ends with `_[i]`, the mark that flame graph tools use for inlined frames. The format is text, so two profiles can be compared with `diff`, and flame graph tools read it.
- Inlining:
  - Inlining by the C compiler is recorded in DWARF under the symbols of the functions, which are Emela paths, so inlined frames appear as their own frames.
  - Inlining by δ in Core IR (`docs/optimization.md`, layers 1 and 5) leaves no C function behind, so a function unfolded there is counted in its caller.
- Use the same interval on every operating system (1000 samples per second with both `perf -F 1000` and Time Profiler), so that numbers from macOS and Linux compare.
- Prototype first: build a program with a small helper whose C function has an assembler label, with `-O2 -g -fno-omit-frame-pointer`, and check that `perf` and `xctrace` report the frame of the helper inlined by the C compiler under the name of its label.
- Later work, not part of this proposal and needing no change of the specification:
  - Carry source spans through Core IR, monomorphisation and LIR, and emit `#line` in the C, so that frames resolve to lines. The spans serve other tools too (a debugger, locations of runtime errors), so this is infrastructure of its own.
  - A page for a browser, written next to `profile.folded` (`profile.html`, with the data and the code of the page in the one file): a flame graph of the folded stacks, and a flame chart that lays the samples out in time. The flame chart needs the time of each sample, which `perf script` and `xctrace export` give and folded stacks drop, so the driver keeps them for the page.
  - Tracing, in a later proposal: points inserted at the start and end of a `with`, of an effect operation and of the life of a fiber, recorded with their times and drawn as spans in the same page. Sampling cannot show these.
  - When δ unfolds a callee, record in the spans of the unfolded body where it was unfolded, so that functions unfolded by δ appear as inlined frames, at any depth of nesting.

## Alternatives

- The format of the profile file. The Chrome `.cpuprofile` keeps the timeline and matches what the JS host writes, but it is a JSON tree that does not compare well with `diff`. pprof has line-level views and profile differences, but it is binary protobuf. Folded stacks answer both uses (which stacks take the samples) with the smallest conversion, and the format can change later because it is not specified.
- Instrumentation: the compiler inserts counters at the entry and exit of each function. It works the same on every target and can count effect operations and fiber switches, but it changes inlining and the cost of small functions, so it cannot show what the optimised code does. It is left for a later proposal, for effects and fibers.
- An experimental feature (`--experimental-profile`): experimental features (`[intro.experimental]`) change the language for a Pome and are read from `Pome.toml` and by the language server. A profiler changes neither, and `profile = true` in `[experimental]` would have no meaning for a dependency or for the editor.
- Leaving it to the user with `EMELA_CC_ARGS`: the symbols still need Emela names, and every user writes the same script to fold the stacks.
- Frames by line in the first version. It needs source spans in every IR, which none has; frames by function need only the symbols of the C functions. Lines are left for later work that does not change the specification.
- Only physical frames (inlined functions counted in their caller), or only logical frames (inlined functions shown as if they were not inlined). Marking inlined frames in one profile serves both uses without a flag.
- An environment variable that names the profiler (`EMELA_PROFILER`), as `EMELA_CC` names the C compiler. The command reads the output of the profiler, so only the profilers it knows could be named, and PATH already chooses their executables. It can be added later without breaking anything.
- A table from the C symbols (`e2_fib`) to Emela paths, written next to the executable, instead of assembler labels. It keeps the emitted C within C11, but only the command reads the table: a profiler started by the user would still show `e2_fib`. With labels, `emela build --profile` is useful with any profiler.
- A flag for the sampling interval. The interval matters for short runs, which the summary detects by the number of samples, and `emela build --profile` lets the user run any profiler with any interval. A flag can be added later without breaking anything.
- The JS target in this proposal. It needs Emela names on the emitted JS functions, the profiler of each JS host (`--cpu-prof` of Node; whether Bun writes the same format is not checked), and a rule for stacks that are cut by suspension on the event loop, none of which the native target has. It is left for a later proposal. Tier 1 targets receive new features first (`[target]`), but `--profile` is a feature of the commands and does not change the meaning of programs, so the native target can have it first.

## Prior art

- MoonBit's `moon run --profile` runs a native build under Time Profiler on macOS and `perf` on Linux.
- `cargo flamegraph` builds with debug information and runs `perf` or `dtrace`.
- `node --cpu-prof` writes a `.cpuprofile` that source maps map back to the original source.

## Open questions
