// Emela のテストランナー（仕様 13章）．テストの起動用モジュールの先頭に埋め込む．
//
// テスト関数を1つずつ呼び，結果を1行ずつ標準出力に書く．行は `\x1eemela-test ` の後に JSON．
// 結果の表示（`test name ... ok`）は呼び出し側（emela-driver）が受け持つ．
// インラインで埋め込むときは行頭の `export ` を取り除くので，`export` は行頭にだけ書く．

const $TEST_EVENT = "\x1eemela-test ";

function $testReport(event) {
  process.stdout.write($TEST_EVENT + JSON.stringify(event) + "\n");
}

// テスト関数の返した値が処理されなかったエラー（14.3）なら，その表示を返す．
// エラーの値の表現（16.5 の戻り値方式）は fail を入れるときに決めるので，今はいつも undefined．
function $testUnhandledError(_value) {
  return undefined;
}

// テスト関数の投げた例外を表示する．defect は `name` が "Defect" の Error（7.5）．
// それ以外の例外（外部関数が投げたもの）も defect として扱い，名前を前に付ける．
function $testDescribe(error) {
  if (error instanceof Error) {
    return error.name === "Defect" ? error.message : `${error.name}: ${error.message}`;
  }
  return String(error);
}

// `tests` は [表示名, 関数] の配列．1つが失敗しても残りを続ける（13.3）．
export async function $runTests(tests) {
  for (const [name, test] of tests) {
    try {
      const value = await test();
      const error = $testUnhandledError(value);
      if (error === undefined) {
        $testReport({ event: "result", name, outcome: "ok" });
      } else {
        $testReport({ event: "result", name, outcome: "error", message: error });
      }
    } catch (error) {
      $testReport({ event: "result", name, outcome: "defect", message: $testDescribe(error) });
    }
  }
  $testReport({ event: "done" });
}
