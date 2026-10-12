// Emela のテストランナー（仕様 13章）．テストの起動用モジュールの先頭に埋め込む．
//
// テスト関数を1つずつ呼び，結果を1行ずつ標準出力に書く．行は `\x1eemela-test ` の後に JSON．
// 結果の表示（`test name ... ok`）は呼び出し側（emela-driver）が受け持つ．
// インラインで埋め込むときは行頭の `export ` を取り除くので，`export` は行頭にだけ書く．

const $TEST_EVENT = "\x1eemela-test ";

function $testReport(event) {
  process.stdout.write($TEST_EVENT + JSON.stringify(event) + "\n");
}

// テスト関数の返した値が処理されなかったエラー（13.3）なら，その表示を返す．
// 失敗はランタイムの `$Fail` で返る（16.5）．$Fail のクラスを読み込まないので `Symbol.for` の鍵で見分ける．
function $testUnhandledError(value) {
  const show = value?.[Symbol.for("emela.fail")];
  return typeof show === "function" ? show.call(value) : undefined;
}

// テスト関数の投げた例外を表示する．defect は `name` が "Defect" の Error（7.5）．
// それ以外の例外（外部関数が投げたもの）も defect として扱い，名前を前に付ける．
function $testDescribe(error) {
  if (error instanceof Error) {
    return error.name === "Defect" ? error.message : `${error.name}: ${error.message}`;
  }
  return String(error);
}

// 中断したまま resume されないテスト（14.5）と，ホストのコールバックの中で投げられた例外を，
// 実行中のテストに結び付ける．`$testCurrent` は実行中のテストの `{ defect, interrupt }`．
let $testCurrent = null;

// イベントループが空になったのに実行中のテストが終わっていなければ，そのテストは中断されたまま．
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

// テスト関数を1つ呼んで結果を返す．中断しうるテスト関数（ジェネレータを返す）は `start`（ランタイムの
// `$start`）で進め，終わるか，defect か，中断されたまま巻き戻るまで待つ．
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
    const defect = (error) => finish({ outcome: "defect", message: $testDescribe(error) });
    $testCurrent = { defect, interrupt: () => finish({ outcome: "interrupted" }) };
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

// `tests` は [表示名, 関数] の配列．1つが失敗しても残りを続ける（13.3）．
// `start` は出力したモジュールが公開するランタイムの `$start`（中断しうるテスト関数がなければ undefined）．
export async function $runTests(tests, start) {
  for (const [name, test] of tests) {
    const result = await $runTest(test, start);
    $testReport({ event: "result", name, ...result });
  }
  $testReport({ event: "done" });
}
