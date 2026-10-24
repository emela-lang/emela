-- Checks the tree-sitter highlighting headlessly: the capture at chosen positions of
-- fixture.emel, and that the Emela files of the repository parse without errors. Run through
-- `run.sh treesitter`, which builds the parser first.

local here = vim.fn.fnamemodify(debug.getinfo(1, "S").source:sub(2), ":p:h")
local root = vim.fn.fnamemodify(here, ":h:h:h")
local failures = {}
local checked = 0

local function fail(message)
  table.insert(failures, message)
end

local function open(path)
  vim.cmd("silent edit! " .. vim.fn.fnameescape(path))
  if vim.bo.filetype ~= "emela" then
    fail(path .. ": filetype is " .. vim.bo.filetype)
  end
  if not vim.treesitter.highlighter.active[vim.api.nvim_get_current_buf()] then
    fail(path .. ": tree-sitter highlighting is not active")
  end
  -- Parsing is asynchronous; finish it before reading captures.
  vim.treesitter.get_parser(0, "emela"):parse(true)
end

-- On the first line containing `line`, the `nth` occurrence of `token` is highlighted with
-- `capture`: the last capture there, which wins.
local function expect(line, token, capture, nth)
  nth = nth or 1
  local where = ("%s: `%s` `%s`"):format(vim.fn.expand("%:t"), line, token)
  local lnum = vim.fn.search("\\V" .. vim.fn.escape(line, "\\"), "nw")
  if lnum == 0 then
    return fail(where .. ": line not found")
  end
  local text = vim.fn.getline(lnum)
  local col = 0
  for _ = 1, nth do
    col = text:find(token, col + 1, true)
    if not col then
      return fail(where .. ": token not found")
    end
  end
  checked = checked + 1
  local captures = vim.treesitter.get_captures_at_pos(0, lnum - 1, col - 1)
  local actual = #captures > 0 and captures[#captures].capture or "(none)"
  if actual ~= capture then
    fail(("%s (#%d, line %d): expected @%s, got @%s"):format(where, nth, lnum, capture, actual))
  end
end

-- The whole buffer parses without ERROR or MISSING nodes.
local function expect_no_errors()
  checked = checked + 1
  local tree = vim.treesitter.get_parser(0, "emela"):parse()[1]
  if not tree:root():has_error() then
    return
  end
  local query = vim.treesitter.query.parse("emela", "(ERROR) @error (MISSING) @missing")
  for _, node in query:iter_captures(tree:root(), 0) do
    local row, col = node:start()
    return fail(("%s:%d:%d: %s"):format(vim.fn.expand("%:."), row + 1, col + 1, node:type()))
  end
  fail(vim.fn.expand("%:.") .. ": has an error")
end

-- ---------------------------------------------------------------------------
open(here .. "/fixture.emel")

expect("import Http.Client", "import", "keyword.import")
expect("import Http.Client", "Http", "module")
expect("import Data.Json.{Json, decode}", "Json", "type", 2)
expect("## A documented type", "##", "comment.documentation")
expect("pub opaque type Email", "pub", "keyword.modifier")
expect("pub opaque type Email", "opaque", "keyword.modifier")
expect("pub opaque type Email", "type", "keyword.type")
expect("pub opaque type Email", "Email", "type.definition")
expect("pub opaque type Email", "value", "variable.member")
expect("pub opaque type Email", "String", "type")
expect("  derive Eq, Show", "derive", "keyword")
expect("type Pair[A, B: Eq + Show]", "A", "type")
expect("type Pair[A, B: Eq + Show]", "+", "operator")
expect("  Circle(radius: Float)", "Circle", "constructor")
expect("error NotFound(id: Int)", "error", "keyword.type")
expect("const MAX_SIZE = 1_000", "MAX_SIZE", "constant")
expect("const MAX_SIZE = 1_000", "1_000", "number")
expect('@external(js, "Math.abs")', "@", "attribute")
expect('@external(js, "Math.abs")', "external", "attribute")
expect('@external(js, "Math.abs")', '"Math.abs"', "string")
expect("fn abs(x: Int) -> Int", "fn", "keyword.function")
expect("fn abs(x: Int) -> Int", "abs", "function")
expect("fn abs(x: Int) -> Int", "x", "variable.parameter")
expect("fn abs(x: Int) -> Int", "->", "operator")
expect("  suspend fn find(id: Int)", "suspend", "keyword.modifier")
expect("  suspend fn find(id: Int)", "fails", "keyword")
expect("  suspend fn find(id: Int)", "|", "operator")
expect("handler PostgresUsers(conn: Connection): Users", "handler", "keyword.type")
expect("handler PostgresUsers(conn: Connection): Users", "Users", "type", 2)
expect("  init {", "init", "keyword")
expect("  release {", "release", "keyword")
expect("    close(self.conn)", "close", "function.call")
expect("    close(self.conn)", "self", "variable.builtin")
expect("    close(self.conn)", "conn", "variable.member")
expect("layer AppLive { PostgresUsers, ConsoleLogger }", "layer", "keyword.type")
expect("impl Shape: Show {", "impl", "keyword")
expect("  fn show(self)", "self", "variable.builtin")
expect("fn run[A, R](f: fn() -> A fails A | B use { A, R }) -> A use R {", "use", "keyword")
expect("  with AppLive { f() }", "with", "keyword")
expect("  with AppLive { f() }", "AppLive", "type")
expect("  hex = 0xFF_ff + 0b1010 + 3.14 + 1.0e9", "3.14", "number.float")
expect("  unit = ()", "(", "punctuation.bracket")
expect("  flags = True && !False", "True", "boolean")
expect("  flags = True && !False", "!", "operator")
expect("  flags = True && !False", "|>", "operator")
expect("  flags = True && !False", "List", "module")
expect("  flags = True && !False", "map", "function.method.call")
expect('  escaped = "tab', "\\t", "string.escape")
expect('  escaped = "tab', "\\u{1F600}", "string.escape")
expect('  nested = "a #{', "#{", "punctuation.special")
expect('  nested = "a #{', "f", "function.call")
expect("    line #{ name }", "name", "variable")
expect('    "quoted" ## not a comment', "##", "string")
expect("  fails = for", "fails", "variable")
expect("  r = x escape {", "escape", "keyword.exception")
expect("  r = x escape {", "NotFound", "constructor")
expect("  r = x escape {", "fail", "keyword.exception")
expect("  assert self == Self", "assert", "keyword")
expect("  waited = Fiber.await(fiber)", "await", "function.method.call")
expect("  greeting = load(id) match {", "match", "keyword.conditional")
expect("    Some(user) -> user.name", "Some", "constructor")
expect("    None ->", "None", "constructor")

-- Files that must parse without errors: everything but the compiler's error cases and the
-- statement-level lexer inputs.
local files = vim.fn.globpath(root, "{examples,crates,tests}/**/*.emel", false, true)
for _, path in ipairs(files) do
  if not path:find("/test_data/parser/err/", 1, true) and not path:find("/test_data/lexer/", 1, true) then
    open(path)
    expect_no_errors()
  end
end

if #failures > 0 then
  for _, failure in ipairs(failures) do
    io.stderr:write(failure .. "\n")
  end
  io.stderr:write(("%d of %d checks failed\n"):format(#failures, checked))
  vim.cmd("cquit 1")
end
io.stdout:write(("tree-sitter: %d checks passed\n"):format(checked))
vim.cmd("quitall!")
