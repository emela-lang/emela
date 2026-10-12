// Env の JS 用のホスト関数．`process.env` を読む．

// 名前 `name` の環境変数が設定されているか．
export const has = (name) => Object.hasOwn(process.env, name);

// 名前 `name` の環境変数の値．`has` が true のときだけ呼ぶ．
export const get = (name) => process.env[name];
