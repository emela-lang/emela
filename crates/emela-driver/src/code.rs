//! Error codes: `E` plus four digits (`W` plus four digits for warnings). A number is never
//! reused once assigned.
//!
//! Ranges by stage: E01xx lexing and parsing, E02xx modules, imports and project layout, E03xx
//! types, E04xx effects, E09xx tooling (I/O, node, cc, ...). The table lives in appendix A of the
//! spec; this is a copy. E01xx codes are owned by emela-syntax's `DiagnosticCode`.

// E02xx: modules, imports and project layout
pub const INVALID_FILE_NAME: &str = "E0201";
pub const INVALID_DIR_NAME: &str = "E0202";
pub const RESERVED_MODULE_NAME: &str = "E0203";
pub const DUPLICATE_MODULE: &str = "E0204";
pub const UNDEFINED_MODULE: &str = "E0205";
pub const IMPORT_CYCLE: &str = "E0206";
pub const NO_PROJECT_FILE: &str = "E0207";
pub const OUTSIDE_SOURCE_ROOT: &str = "E0208";
pub const ENTRY_NOT_FOUND: &str = "E0209";
pub const ENTRY_NOT_MODULE: &str = "E0210";
// Name resolution
pub const UNDEFINED_NAME: &str = "E0211";
pub const UNDEFINED_MEMBER: &str = "E0212";
pub const DUPLICATE_DEFINITION: &str = "E0213";
pub const PRIVATE_ITEM: &str = "E0214";
pub const OPAQUE_CONSTRUCTION: &str = "E0215";
pub const OPAQUE_PATTERN: &str = "E0216";
pub const WRONG_KIND_OF_NAME: &str = "E0217";
pub const AMBIGUOUS_NAME: &str = "E0218";
pub const SELF_OUTSIDE: &str = "E0219";
pub const DUPLICATE_BINDING: &str = "E0220";
pub const EFFECT_AS_TYPE: &str = "E0221";
pub const INVALID_UPDATE_TARGET: &str = "E0222";
pub const POSITIONAL_IN_UPDATE: &str = "E0223";
pub const INTRINSIC_OUTSIDE_CORE: &str = "E0224";
pub const MISPLACED_INTRINSIC: &str = "E0225";
// Tests (spec 13.1)
pub const INVALID_TEST: &str = "E0226";
pub const TEST_FN_OUTSIDE_TESTS: &str = "E0227";
// fns visible only inside core (`@internal`)
pub const INTERNAL_FN_OUTSIDE_CORE: &str = "E0228";
pub const MISPLACED_INTERNAL: &str = "E0229";
// The `emela run` entry has no main (spec 8.3)
pub const ENTRY_WITHOUT_MAIN: &str = "E0230";
// Consts refer to each other by name only (spec 5.6)
pub const CONST_CYCLE: &str = "E0231";
// `[build].target` of Pome.toml (spec 4.7)
pub const UNKNOWN_BUILD_TARGET: &str = "E0232";
/// `@external` in a dependency that `[external] allow` does not allow (spec 4.7, chapter 12).
pub const EXTERNAL_IN_DEPENDENCY: &str = "E0233";
/// A dependency needs a newer compiler (`[pome].emela`, spec 4.7).
pub const NEWER_COMPILER_NEEDED: &str = "E0234";
/// A root name of a dependency that cannot be a module name (spec 4.7).
pub const INVALID_ROOT_NAME: &str = "E0235";
/// The dependencies of `Pome.toml` cannot be used: no lock, or not in the cache for the language
/// server (spec 4.7).
pub const DEPENDENCIES_NOT_READY: &str = "E0236";
/// A `pub` `@external` fn in a dependency (spec 4.7).
pub const PUB_EXTERNAL_IN_DEPENDENCY: &str = "E0237";
/// A dependency whose `[pome].targets` does not include the build target (spec 4.7).
pub const UNSUPPORTED_TARGET: &str = "E0238";
/// A module of the source root imports a module of the test root or a `[dev-dependencies]` Pome
/// (spec 4.7, 13.1).
pub const TEST_ONLY_MODULE: &str = "E0239";
pub const SHADOWED_BY_TYPE_PARAM: &str = "W0201";
pub const SHADOWS_PRELUDE: &str = "W0202";
/// A dependency written for an older minor version of the compiler (`[pome].emela`, spec 4.7).
pub const OLDER_COMPILER_DEPENDENCY: &str = "W0203";
/// A Pome in `[external] allow` that is not depended on (spec 4.7).
pub const UNUSED_EXTERNAL_ALLOW: &str = "W0204";
/// `[pome].license` is not a string or not an SPDX license expression (spec `[pome.manifest]`).
pub const LICENSE_NOT_AN_EXPRESSION: &str = "W0205";
/// An experimental feature named in `[experimental]` or by a flag `--experimental-<name>` is
/// unknown, stable or removed (spec `[intro.experimental]`).
pub const UNKNOWN_EXPERIMENTAL_FEATURE: &str = "W0206";
/// A value of `[experimental]` is not a boolean (spec `[pome.manifest]`).
pub const EXPERIMENTAL_NOT_A_BOOLEAN: &str = "W0207";

// E03xx: types
pub const TYPE_MISMATCH: &str = "E0301";
pub const INFINITE_TYPE: &str = "E0302";
pub const NON_EXHAUSTIVE_MATCH: &str = "E0303";
pub const WRONG_TYPE_ARG_COUNT: &str = "E0304";
pub const WRONG_ARG_COUNT: &str = "E0305";
pub const UNKNOWN_LABEL: &str = "E0306";
pub const DUPLICATE_ARG: &str = "E0307";
pub const MISSING_PATTERN_FIELDS: &str = "E0308";
pub const PATTERN_FIELD_COUNT: &str = "E0309";
pub const NOT_NUMERIC: &str = "E0310";
pub const INT_LITERAL_OUT_OF_RANGE: &str = "E0311";
pub const NO_FIELD: &str = "E0312";
pub const NO_FIELDS: &str = "E0313";
pub const FIELD_ON_UNKNOWN_TYPE: &str = "E0314";
pub const OPAQUE_FIELD: &str = "E0315";
pub const MISSING_PARAM_TYPE: &str = "E0316";
pub const REFUTABLE_BINDING: &str = "E0317";
// Errors (spec chapter 7)
pub const UNHANDLED_ERRORS: &str = "E0318";
pub const NOT_AN_ERROR: &str = "E0319";
pub const INVALID_ESCAPE_ARM: &str = "E0320";
pub const ERROR_SET_MISMATCH: &str = "E0321";
// Constraints on effects (spec 9.5)
pub const IMMEDIATE_ON_NON_EFFECT: &str = "E0322";
// Sets as declaration type arguments (spec 9.2)
pub const SET_ARG_MISMATCH: &str = "E0323";
pub const SET_AS_TYPE: &str = "E0324";
// Required trait methods (spec 10.2)
pub const INCOMPLETE_IMPL: &str = "E0325";
// Traits (spec 10.1–10.3)
pub const NOT_IMPLEMENTED: &str = "E0326";
// E0327 is retired: only `@export` fns are exported, and they take only boundary types
// (`[ext.export]`).
/// Found by name resolution, which sees the names in the head.
pub const IMPL_HEAD: &str = "E0328";
pub const SUPERTRAIT_CYCLE: &str = "E0329";
pub const DUPLICATE_IMPL: &str = "E0330";
pub const ORPHAN_IMPL: &str = "E0331";
pub const IMPL_FN_PARAM_COUNT: &str = "E0332";
pub const AMBIGUOUS_SELF: &str = "E0333";
// Method calls (`[type.rules]`, `[data.type]`). E0334 is found by name resolution, which sees the
// declarations.
pub const FN_NAMED_LIKE_FIELD: &str = "E0334";
pub const AMBIGUOUS_METHOD: &str = "E0335";
// Derivations of std (`[trait.derive]`) and their field attributes (`[kw.annotation]`).
pub const OPAQUE_DERIVE: &str = "E0336";
pub const DUPLICATE_KEY: &str = "E0337";
pub const MISPLACED_KEY_TAG: &str = "E0338";
pub const OBJECT_KEY_FORM: &str = "E0339";
// Built-in traits determined automatically (`[trait.builtin]`).
pub const CAPABILITY_ON_SET: &str = "E0340";
pub const AUTO_TRAIT_IMPL: &str = "E0341";
pub const UNREACHABLE_MATCH_ARM: &str = "W0301";
pub const UNRAISED_ERROR: &str = "W0302";
pub const UNUSED_DERIVE: &str = "W0303";
/// `[dyn.error]`: the entry's `main` returns a type other than `()`, `ExitCode` and Never.
pub const MAIN_VALUE_DISCARDED: &str = "W0304";
/// `[fn.decl]`: a type parameter of a fn does not appear in the type of the fn.
pub const UNUSED_TYPE_PARAM: &str = "W0305";
pub const UNUSED_LABEL_PROVIDER: &str = "W0401";
// W0402 is retired: a capability that leaves the `with` that made it is E0455
// (`[effect.escape]`).
/// `[effect.use]`: `use E` written as an expression is deprecated.
pub const DEPRECATED_USE: &str = "W0403";

// E04xx: effects and handlers (spec chapter 8)
pub const UNKNOWN_OPERATION: &str = "E0401";
pub const UNHANDLED_EFFECTS: &str = "E0402";
pub const EFFECT_SET_MISMATCH: &str = "E0403";
pub const MISSING_HANDLER_OPS: &str = "E0404";
pub const DUPLICATE_HANDLER_OP: &str = "E0405";
pub const HANDLER_OP_ARG_COUNT: &str = "E0406";
pub const EXTERNAL_OUTSIDE_HANDLER: &str = "E0407";
// E0408 to E0410 are retired: a capability value can be passed as an argument, stored in a data
// structure and returned, since `use E` became a type (`[effect.use]`).
pub const HANDLER_CYCLE: &str = "E0411";
// External functions (spec 12.1). A side-effect path like effects, so they live in E04xx
// (E05xx to E08xx are reserved).
pub const MISSING_EXTERNAL_TARGET: &str = "E0412";
// suspend and host effects (`[effect.async]`, `[effect.host]`, spec 9.5)
pub const HOST_EFFECT_HANDLER: &str = "E0413";
pub const USE_HOST_EFFECT: &str = "E0414";
pub const SUSPEND_MISMATCH: &str = "E0415";
pub const NOT_IMMEDIATE: &str = "E0416";
pub const SUSPENDING_FN_OP: &str = "E0417";
// Stateful handlers and layers (spec 8.4, 8.5)
pub const HANDLER_CTOR_OUTSIDE_INIT: &str = "E0418";
pub const MISSING_INIT: &str = "E0419";
pub const RELEASE_FAILS: &str = "E0420";
pub const DUPLICATE_LAYER_EFFECT: &str = "E0421";
pub const LAYER_CYCLE: &str = "E0422";
// Effect type parameters (spec 8.1)
pub const MISSING_EFFECT_TYPE_ARGS: &str = "E0423";
pub const EFFECT_PARAM_NOT_TYPE: &str = "E0424";
// E0425 is retired: a task is a capability-bound type of its group, so one that leaves the with
// of its group is E0455 (`[effect.escape]`).
pub const OP_TYPE_PARAMS: &str = "E0426";
// External functions the native target does not support yet (spec 12.4). E0428 is retired: `suspend
// fn` external functions on C complete their task later (spec 12.6).
pub const CALLBACK_EXTERNAL: &str = "E0427";
// Labels (issue #255)
pub const LABEL_BASE: &str = "E0429";
pub const NOT_A_LABEL: &str = "E0430";
pub const LABEL_MISMATCH: &str = "E0431";
pub const LABEL_PROVIDED_TWICE: &str = "E0432";
pub const HANDLER_OF_LABEL: &str = "E0433";
// Arguments of handler init (spec 8.4)
pub const MISSING_INIT_ARGS: &str = "E0434";
pub const UNEXPECTED_INIT_ARGS: &str = "E0435";
// Exported functions and entry adapters (`[ext.export]`, `[ext.entry]`)
pub const NOT_BOUNDARY_EXPORT: &str = "E0436";
pub const EXPORT_OUTSIDE_ENTRY: &str = "E0437";
pub const DUPLICATE_EXPORT: &str = "E0438";
pub const NOT_ENTRY_ADAPTER: &str = "E0439";
pub const ENTRY_MISMATCH: &str = "E0440";
pub const SUSPENDING_EXPORT: &str = "E0441";

// HTTP services (`[lib.http-service]`)
pub const HTTP_SERVICE_ON_C: &str = "E0442";
// Bytes and Arrays at the boundary on the native target (`[ext.boundary]`)
pub const BOUNDARY_TYPE_ON_C: &str = "E0443";
// E0444 is retired with the experimental feature `capture-checking` (`[effect.escape]`).
// Host effects (`[effect.host]`): the uses of an external function (`[ext.call]`) and effects
// without operations (`[effect.decl]`)
pub const EXTERNAL_USES: &str = "E0445";
pub const EFFECT_WITHOUT_OPS: &str = "E0446";
// Handler members (`[effect.decl]`, `[effect.with]`, `[effect.handler]`)
pub const MEMBER_ITEM_FORM: &str = "E0447";
pub const MEMBER_OUTSIDE_INIT: &str = "E0448";
pub const INNER_HANDLER_MISMATCH: &str = "E0449";
// Handlers of several effects (`[effect.handler]`, `[effect.use]`)
pub const SHARED_OPERATION_NAME: &str = "E0450";
pub const REPEATED_HANDLER_EFFECT: &str = "E0451";
pub const MISSING_CAPABILITY_EFFECTS: &str = "E0452";
// Effect aliases (`[effect.use]`) and callbacks to the host (`[ext.boundary]`)
pub const EFFECT_ALIAS_MISUSE: &str = "E0453";
pub const CALLBACK_EFFECT: &str = "E0454";
// Capabilities told apart in types and in R (`[effect.escape]`, `[effect.handler]`,
// `[ext.boundary]`, `[type.rules]`)
pub const CAPABILITY_LEAK: &str = "E0455";
pub const CAPABILITY_IN_OP_RESULT: &str = "E0456";
pub const BORROWED_POSITION: &str = "E0457";
pub const BOUND_TYPE_OUTSIDE_HANDLER: &str = "E0458";
// Handlers with type parameters (`[effect.handler]`, `[effect.with]`)
pub const HANDLER_ARGS_UNKNOWN: &str = "E0459";
pub const INNER_HANDLER_TYPE_PARAMS: &str = "E0460";
// No suspension after resume (`[effect.handler]`, `[dyn.suspend]`)
pub const SUSPEND_AFTER_RESUME: &str = "E0461";
pub const UNUSED_HANDLER_PARAM: &str = "E0462";

// E09xx: tooling
pub const PATH_NOT_FOUND: &str = "E0901";
pub const CANNOT_READ: &str = "E0902";
pub const CANNOT_WRITE_OUTPUT: &str = "E0903";
/// The JS host could not be started, or is older than the JS target supports for it
/// (`--target js`, `[cmd.build]`).
pub const NODE_UNAVAILABLE: &str = "E0904";
pub const NO_JS_BACKEND: &str = "E0905";
pub const INVALID_JS_OUTPUT: &str = "E0906";
// The C compiler of the native target (`[target.native.cc]`)
pub const CC_UNAVAILABLE: &str = "E0907";
pub const CC_FAILED: &str = "E0908";
/// The `wasmtime` executable could not be started, or is too old (`--target wasm`).
pub const WASMTIME_UNAVAILABLE: &str = "E0909";
/// `EMELA_NODE` was read to choose the JS host (`[cmd.build]`).
pub const NODE_ENV_DEPRECATED: &str = "W0902";
// W0903 is retired with the build target `c` in 0.29 (`[target.native]`).
