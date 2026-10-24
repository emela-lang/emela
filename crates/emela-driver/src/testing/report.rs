//! Test results and the reporters that display them (`[cmd.test.reporter]`, `[cmd.test.output]`).

use std::ffi::OsString;
use std::io::{self, Write};
use std::time::{Duration, Instant};

/// The exit code when any test failed.
pub const TEST_FAILURE_EXIT_CODE: u8 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestResult {
    pub name: String,
    pub outcome: TestOutcome,
    /// Where the failure happened, as `<path>:<line>:<column>`: only a defect from a failed
    /// assert has one (`[cmd.test.outcome]`).
    pub location: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestOutcome {
    Passed,
    /// Stopped by a defect (spec 7.5). The value is the defect message.
    Defect(String),
    /// Ended with an unhandled error (spec 13.3). The value is the error shown.
    Error(String),
    /// Unwound while suspended without a resume (spec 14.5).
    Interrupted,
    /// The test runner (node) exited during this test. `code` is node's exit code, and `not_run`
    /// the number of tests after it that never ran.
    Aborted {
        code: i32,
        not_run: usize,
    },
}

impl TestOutcome {
    pub fn is_passed(&self) -> bool {
        matches!(self, TestOutcome::Passed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestReport {
    /// In run order.
    pub results: Vec<TestResult>,
    /// Tests excluded by the filter.
    pub filtered_out: usize,
    /// node's stderr (`dbg` output and such), only with [`crate::Output::Capture`].
    pub stderr: Vec<u8>,
}

impl TestReport {
    pub fn passed(&self) -> usize {
        self.results
            .iter()
            .filter(|r| r.outcome.is_passed())
            .count()
    }

    pub fn failed(&self) -> usize {
        self.results.len() - self.passed()
    }

    pub fn is_ok(&self) -> bool {
        self.failed() == 0
    }

    /// 0 if nothing failed, otherwise [`TEST_FAILURE_EXIT_CODE`].
    pub fn exit_code(&self) -> u8 {
        if self.is_ok() {
            0
        } else {
            TEST_FAILURE_EXIT_CODE
        }
    }
}

/// How `emela test` displays its results (`[cmd.test.reporter]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Reporter {
    /// For a person at a terminal. The layout is not specified, so it may change freely.
    Pretty,
    /// The line format of `[cmd.test.output]`.
    #[default]
    Plain,
    /// Only the failures and the summary line, for a coding agent.
    Agent,
}

impl Reporter {
    /// The reporter when `--reporter` is omitted: `agent` inside a coding agent ([`runs_in_agent`]),
    /// else `pretty` when stdout is a terminal, else `plain`.
    pub fn detect(in_agent: bool, stdout_is_terminal: bool) -> Reporter {
        if in_agent {
            Reporter::Agent
        } else if stdout_is_terminal {
            Reporter::Pretty
        } else {
            Reporter::Plain
        }
    }
}

/// Environment variables that mean the command runs inside a coding agent when set and not empty.
/// `AI_AGENT` is the one `[cmd.test.reporter]` names; the others are set by particular agents in
/// the processes they start. Variables that a person's shell may also have (`REPL_ID`,
/// `GOOSE_PROVIDER`) and guesses from the values of `PATH`, `EDITOR` or `TERM_PROGRAM` are left
/// out, so that a person at a terminal is not taken for an agent.
pub const AGENT_ENV_VARS: &[&str] = &[
    "AI_AGENT",
    "CLAUDECODE",
    "CLAUDE_CODE",
    "GEMINI_CLI",
    "CODEX_SANDBOX",
    "CODEX_THREAD_ID",
    "OPENCODE",
    "AUGMENT_AGENT",
    "JUNIE_DATA",
    "COPILOT_AGENT",
    "COPILOT_CLI",
    "CURSOR_AGENT",
];

/// Whether the command runs inside a coding agent, reading the environment through `var`
/// (`std::env::var_os`).
pub fn runs_in_agent(var: impl Fn(&str) -> Option<OsString>) -> bool {
    AGENT_ENV_VARS
        .iter()
        .any(|name| var(name).is_some_and(|value| !value.is_empty()))
}

/// Writes results with a [`Reporter`] as the test runner reports them. Write errors on `out` are
/// ignored: a closed stdout does not change which tests pass.
pub(crate) struct Display<'a> {
    reporter: Reporter,
    /// Only for [`Reporter::Pretty`].
    color: bool,
    out: &'a mut dyn Write,
    /// What the program wrote to stdout since the last result line. Tests run one at a time, so
    /// with [`Reporter::Agent`] these lines belong to the test that is running.
    output: Vec<u8>,
    /// The number of failures printed so far ([`Reporter::Agent`]).
    failures: usize,
    /// The width of the name column ([`Reporter::Pretty`]).
    width: usize,
    started: Instant,
    /// When the last result arrived. The elapsed time of a test is the time between the arrival
    /// of the result before it and its own, so the runtimes need not measure it. The first test's
    /// time includes starting the runtime, which only `pretty` shows.
    last: Instant,
}

impl<'a> Display<'a> {
    pub(crate) fn new(reporter: Reporter, color: bool, out: &'a mut dyn Write) -> Self {
        let now = Instant::now();
        Display {
            reporter,
            color,
            out,
            output: Vec::new(),
            failures: 0,
            width: 0,
            started: now,
            last: now,
        }
    }

    /// Called once before the tests start, with the names of the tests that will run.
    pub(crate) fn start(&mut self, names: &[String]) {
        let now = Instant::now();
        self.started = now;
        self.last = now;
        match self.reporter {
            Reporter::Plain => {
                let _ = write_header(self.out, names.len());
            }
            Reporter::Pretty => {
                self.width = names.iter().map(|n| n.chars().count()).max().unwrap_or(0);
            }
            Reporter::Agent => {}
        }
    }

    /// A stdout line of the program that is not a result line.
    pub(crate) fn output(&mut self, line: &[u8]) {
        match self.reporter {
            Reporter::Plain | Reporter::Pretty => {
                let _ = self.out.write_all(line);
            }
            Reporter::Agent => self.output.extend_from_slice(line),
        }
    }

    pub(crate) fn result(&mut self, result: &TestResult) {
        let now = Instant::now();
        let elapsed = now - self.last;
        self.last = now;
        let output = std::mem::take(&mut self.output);
        let _ = match self.reporter {
            Reporter::Plain => write_result(self.out, result),
            Reporter::Pretty => self.write_pretty_result(result, elapsed),
            Reporter::Agent if result.outcome.is_passed() => Ok(()),
            Reporter::Agent => self.write_agent_failure(result, &output),
        };
    }

    /// Called once after the last result.
    pub(crate) fn finish(&mut self, report: &TestReport) {
        let _ = match self.reporter {
            Reporter::Plain => write_summary(self.out, report),
            Reporter::Pretty => self.write_pretty_summary(report),
            Reporter::Agent => {
                let blank = if self.failures > 0 { "\n" } else { "" };
                write!(self.out, "{blank}").and_then(|()| write_summary_line(self.out, report))
            }
        };
        let _ = self.out.flush();
    }

    /// `FAIL <name>` (with ` <location>` when the failure has one), the details, and what the test
    /// wrote to stdout.
    fn write_agent_failure(&mut self, result: &TestResult, output: &[u8]) -> io::Result<()> {
        if self.failures > 0 {
            writeln!(self.out)?;
        }
        self.failures += 1;
        match &result.location {
            Some(location) => writeln!(self.out, "FAIL {} {location}", result.name)?,
            None => writeln!(self.out, "FAIL {}", result.name)?,
        }
        write_details(self.out, &result.outcome)?;
        self.out.write_all(output)?;
        if !output.is_empty() && !output.ends_with(b"\n") {
            writeln!(self.out)?;
        }
        self.out.flush()
    }

    fn write_pretty_result(&mut self, result: &TestResult, elapsed: Duration) -> io::Result<()> {
        let (mark, color) = if result.outcome.is_passed() {
            ("✓", GREEN)
        } else {
            ("×", RED)
        };
        let mark = self.paint(color, mark);
        let time = self.paint(DIM, &format!("{}ms", elapsed.as_millis()));
        let width = self.width;
        writeln!(self.out, "   {mark} {:<width$}  {time}", result.name)?;
        self.out.flush()
    }

    fn write_pretty_summary(&mut self, report: &TestReport) -> io::Result<()> {
        for failure in report.results.iter().filter(|r| !r.outcome.is_passed()) {
            let heading = self.paint(RED, &format!("──── FAIL  {} ────", failure.name));
            writeln!(self.out, "\n{heading}")?;
            if let Some(location) = &failure.location {
                let location = self.paint(DIM, location);
                writeln!(self.out, " --> {location}")?;
            }
            match &failure.outcome {
                TestOutcome::Defect(message) => self.write_pretty_defect(message)?,
                outcome => write_details(self.out, outcome)?,
            }
        }
        let mut counts = Vec::new();
        if report.failed() > 0 {
            counts.push(self.paint(RED, &format!("{} failed", report.failed())));
        }
        if report.passed() > 0 || report.failed() == 0 {
            counts.push(self.paint(GREEN, &format!("{} passed", report.passed())));
        }
        if report.filtered_out > 0 {
            counts.push(format!("{} filtered out", report.filtered_out));
        }
        let label = self.paint(BOLD, " Tests ");
        writeln!(
            self.out,
            "\n{label} {} ({})",
            counts.join(" | "),
            report.results.len()
        )?;
        let label = self.paint(BOLD, " Duration ");
        writeln!(self.out, "{label} {}ms", self.started.elapsed().as_millis())
    }

    /// The details of a defect, with the place where the two sides of a comparison assert
    /// differ marked under the right side.
    fn write_pretty_defect(&mut self, message: &str) -> io::Result<()> {
        let lines: Vec<&str> = message.lines().collect();
        let sides = lines
            .windows(2)
            .position(|pair| pair[0].starts_with(LEFT_PREFIX) && pair[1].starts_with(RIGHT_PREFIX));
        let Some(at) = sides.filter(|&at| at > 0) else {
            return writeln!(self.out, "defect: {message}");
        };
        writeln!(self.out, "defect: {}", lines[..at].join("\n"))?;
        let left = &lines[at][LEFT_PREFIX.len()..];
        let right = &lines[at + 1][RIGHT_PREFIX.len()..];
        let left_line = self.paint(RED, &format!("  - left:  {left}"));
        let right_line = self.paint(GREEN, &format!("  + right: {right}"));
        writeln!(self.out, "{left_line}\n{right_line}")?;
        let differs = left
            .chars()
            .zip(right.chars())
            .position(|(l, r)| l != r)
            .or_else(|| {
                let (l, r) = (left.chars().count(), right.chars().count());
                (l != r).then(|| l.min(r))
            });
        if let Some(column) = differs {
            let indent = " ".repeat("  + right: ".len() + column);
            writeln!(self.out, "{indent}{}", self.paint(BOLD, "^"))?;
        }
        for line in &lines[at + 2..] {
            writeln!(self.out, "{line}")?;
        }
        Ok(())
    }

    fn paint(&self, code: &str, text: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_owned()
        }
    }
}

/// How the runtimes lay out the two sides of a failed comparison assert (`[cmd.test.outcome]`).
const LEFT_PREFIX: &str = "  left: ";
const RIGHT_PREFIX: &str = " right: ";

const RED: &str = "31";
const GREEN: &str = "32";
const BOLD: &str = "1";
const DIM: &str = "2";

/// `running 3 tests`．
fn write_header(out: &mut dyn Write, count: usize) -> io::Result<()> {
    let plural = if count == 1 { "" } else { "s" };
    writeln!(out, "running {count} test{plural}")
}

/// `test name ... ok`．
fn write_result(out: &mut dyn Write, result: &TestResult) -> io::Result<()> {
    let status = if result.outcome.is_passed() {
        "ok"
    } else {
        "FAILED"
    };
    writeln!(out, "test {} ... {status}", result.name)?;
    out.flush()
}

/// The details of a failure (`[cmd.test.outcome]`).
fn write_details(out: &mut dyn Write, outcome: &TestOutcome) -> io::Result<()> {
    match outcome {
        TestOutcome::Passed => unreachable!(),
        TestOutcome::Defect(message) => writeln!(out, "defect: {message}"),
        TestOutcome::Error(message) => writeln!(out, "error: {message}"),
        TestOutcome::Interrupted => writeln!(out, "interrupted"),
        TestOutcome::Aborted { code, not_run } => {
            write!(
                out,
                "the test runner exited with code {code} while running this test"
            )?;
            match not_run {
                0 => writeln!(out),
                1 => writeln!(out, "; 1 test after it did not run"),
                n => writeln!(out, "; {n} tests after it did not run"),
            }
        }
    }
}

/// Failure details and the final line of `plain`.
fn write_summary(out: &mut dyn Write, report: &TestReport) -> io::Result<()> {
    let failures: Vec<&TestResult> = report
        .results
        .iter()
        .filter(|r| !r.outcome.is_passed())
        .collect();
    if !failures.is_empty() {
        writeln!(out, "\nfailures:")?;
        for failure in &failures {
            writeln!(out, "\n---- {} ----", failure.name)?;
            if let Some(location) = &failure.location {
                writeln!(out, " --> {location}")?;
            }
            write_details(out, &failure.outcome)?;
        }
        writeln!(out, "\nfailures:")?;
        for failure in &failures {
            writeln!(out, "    {}", failure.name)?;
        }
    }
    writeln!(out)?;
    write_summary_line(out, report)?;
    out.flush()
}

/// `test result: ok. 1 passed; 0 failed; 0 filtered out`, shared by `plain` and `agent`.
fn write_summary_line(out: &mut dyn Write, report: &TestReport) -> io::Result<()> {
    let status = if report.is_ok() { "ok" } else { "FAILED" };
    writeln!(
        out,
        "test result: {status}. {} passed; {} failed; {} filtered out",
        report.passed(),
        report.failed(),
        report.filtered_out
    )
}
