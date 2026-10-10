//! テストの結果とその表示．rustc の `cargo test` に近い形にする．

use std::io::{self, Write};

/// テストに失敗があったときの終了コード．
pub const TEST_FAILURE_EXIT_CODE: u8 = 1;

/// 1つのテストの結果．
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestResult {
    pub name: String,
    pub outcome: TestOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestOutcome {
    Passed,
    /// defect で止まった（7.5）．値は defect のメッセージ．
    Defect(String),
    /// 処理されなかったエラーで終わった（13.3）．値はエラーの値の表示．fail が入ってから使う．
    Error(String),
    /// このテストの途中でテストランナー（node）が終わった．`code` は node の終了コード，
    /// `not_run` はその後ろで実行されなかったテストの数．
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

/// `emela test` の結果．
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestReport {
    /// 実行した順．
    pub results: Vec<TestResult>,
    /// フィルタで外したテストの数．
    pub filtered_out: usize,
    /// [`crate::Output::Capture`] のときだけ，node の標準エラー（`dbg` の出力など）が入る．
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

    /// 失敗がなければ 0，あれば [`TEST_FAILURE_EXIT_CODE`]．
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

/// 失敗の詳細と最後の1行．
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
            match &failure.outcome {
                TestOutcome::Passed => unreachable!(),
                TestOutcome::Defect(message) => writeln!(out, "defect: {message}")?,
                TestOutcome::Error(message) => writeln!(out, "error: {message}")?,
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
