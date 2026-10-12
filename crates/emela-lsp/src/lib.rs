//! 言語サーバー．ホバー，診断，コードアクション．
//!
//! 解析は `emela_driver` に任せ，ここでは LSP のメッセージとの変換だけを持つ．
//! 今の段は診断と整形とホバー，定義へ移動，参照の検索，文書のシンボル．開いているファイルの未保存の内容を重ねて，`emela check` と同じ診断を出し，`emela fmt` と同じく整形し，型と E（エラーの集合）を見せ，名前の定義と参照をたどる．

mod convert;
mod overlay;
mod server;

pub use convert::{SOURCE, path_to_uri, position, uri_to_path};
pub use overlay::Overlay;
pub use server::{Error, capabilities, serve};

use emela_driver::{Frontend, OsFs};
use lsp_server::Connection;

/// 標準入出力で言語サーバーを動かす（`emela lsp`）．exit を受けたら戻る．
pub fn run_stdio<F: Frontend>(frontend: impl FnMut() -> F) -> Result<(), Error> {
    let (connection, io_threads) = Connection::stdio();
    serve(&connection, OsFs, frontend)?;
    drop(connection);
    io_threads.join()?;
    Ok(())
}
