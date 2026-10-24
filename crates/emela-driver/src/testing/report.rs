//! Test results and their output, close to `cargo test`.

use std::io::{self, Write};

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

/// `running 3 tests`．
pub(crate) fn write_header(out: &mut dyn Write, count: usize) -> io::Result<()> {
    let plural = if count == 1 { "" } else { "s" };
    writeln!(out, "running {count} test{plural}")
}

/// `test name ... ok`．
pub(crate) fn write_result(out: &mut dyn Write, result: &TestResult) -> io::Result<()> {
    let status = if result.outcome.is_passed() {
        "ok"
    } else {
        "FAILED"
    };
    writeln!(out, "test {} ... {status}", result.name)?;
    out.flush()
}

/// Failure details and the final line.
pub(crate) fn write_summary(out: &mut dyn Write, report: &TestReport) -> io::Result<()> {
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
            match &failure.outcome {
                TestOutcome::Passed => unreachable!(),
                TestOutcome::Defect(message) => writeln!(out, "defect: {message}")?,
                TestOutcome::Error(message) => writeln!(out, "error: {message}")?,
                TestOutcome::Interrupted => writeln!(out, "interrupted")?,
                TestOutcome::Aborted { code, not_run } => {
                    write!(
                        out,
                        "the test runner exited with code {code} while running this test"
                    )?;
                    match not_run {
                        0 => writeln!(out)?,
                        1 => writeln!(out, "; 1 test after it did not run")?,
                        n => writeln!(out, "; {n} tests after it did not run")?,
                    }
                }
            }
        }
        writeln!(out, "\nfailures:")?;
        for failure in &failures {
            writeln!(out, "    {}", failure.name)?;
        }
    }
    let status = if report.is_ok() { "ok" } else { "FAILED" };
    writeln!(
        out,
        "\ntest result: {status}. {} passed; {} failed; {} filtered out",
        report.passed(),
        report.failed(),
        report.filtered_out
    )?;
    out.flush()
}
