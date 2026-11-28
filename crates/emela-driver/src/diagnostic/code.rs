//! Diagnostic codes: `E` plus four digits (`W` plus four digits for warnings). A number is never
//! reused once assigned.
//!
//! The first digit names the stage and the second the subject (spec `[diag.bands]`): E1xxx
//! lexing and parsing, E2xxx modules, names and projects, E3xxx types, E4xxx effects, E5xxx
//! external functions and targets, E9xxx tools. A code names one kind of mistake, so one
//! constant here may cover several forms that the message tells apart. The table lives in
//! appendix A of the spec (`[diag.list]`); this is a copy. E1xxx codes are owned by
//! emela-syntax's `DiagnosticCode`, except `MISPLACED_TYPE_PARAMS`, which the type checker shares.

use emela_syntax::DiagnosticCode;

// E12xx: syntax of declarations
/// Type parameters where they cannot be written: an operation of a user-defined effect
/// (spec 8.1) or an inner handler (spec 8.4). The parser reports those on a local fn with the
/// same code.
pub const MISPLACED_TYPE_PARAMS: &str = DiagnosticCode::MisplacedTypeParams.as_str();

// E20xx: files and modules
/// A file name or a directory name does not form a module name.
pub const NOT_A_MODULE_NAME: &str = "E2001";
pub const RESERVED_MODULE_NAME: &str = "E2002";
pub const DUPLICATE_MODULE: &str = "E2003";
pub const UNDEFINED_MODULE: &str = "E2004";
pub const IMPORT_CYCLE: &str = "E2005";
pub const OUTSIDE_SOURCE_ROOT: &str = "E2006";
pub const ENTRY_NOT_FOUND: &str = "E2007";
pub const ENTRY_NOT_MODULE: &str = "E2008";
pub const ENTRY_WITHOUT_MAIN: &str = "E2009";
/// A module of the source root imports a module of the test root or a `[dev-dependencies]` Pome
/// (spec 4.7, 13.1).
pub const TEST_ONLY_MODULE: &str = "E2010";

// E21xx: name resolution
pub const UNDEFINED_NAME: &str = "E2101";
pub const UNDEFINED_MEMBER: &str = "E2102";
pub const DUPLICATE_DEFINITION: &str = "E2103";
pub const PRIVATE_ITEM: &str = "E2104";
/// An opaque type is constructed, matched or accessed outside its defining module (spec 4.4).
pub const OPAQUE_OUTSIDE_MODULE: &str = "E2105";
pub const WRONG_KIND_OF_NAME: &str = "E2106";
pub const AMBIGUOUS_NAME: &str = "E2107";
pub const SELF_OUTSIDE: &str = "E2108";
pub const DUPLICATE_BINDING: &str = "E2109";
pub const EFFECT_AS_TYPE: &str = "E2110";
pub const INVALID_UPDATE_TARGET: &str = "E2111";
pub const POSITIONAL_IN_UPDATE: &str = "E2112";
pub const CONST_CYCLE: &str = "E2113";
pub const SHADOWED_BY_TYPE_PARAM: &str = "W2101";
pub const SHADOWS_PRELUDE: &str = "W2102";

// E22xx: definitions restricted to the bundled core or to tests
pub const INTRINSIC_OUTSIDE_CORE: &str = "E2201";
pub const MISPLACED_INTRINSIC: &str = "E2202";
pub const INVALID_TEST: &str = "E2203";
pub const TEST_FN_OUTSIDE_TESTS: &str = "E2204";
pub const INTERNAL_FN_OUTSIDE_CORE: &str = "E2205";
pub const MISPLACED_INTERNAL: &str = "E2206";

// E23xx: `Pome.toml` and dependencies
pub const NO_PROJECT_FILE: &str = "E2301";
pub const UNKNOWN_BUILD_TARGET: &str = "E2302";
/// `@external` in a dependency that `[external] allow` does not allow (spec 4.7, chapter 12).
pub const EXTERNAL_IN_DEPENDENCY: &str = "E2303";
/// A dependency needs a newer compiler (`[pome].emela`, spec 4.7).
pub const NEWER_COMPILER_NEEDED: &str = "E2304";
/// A root name of a dependency that cannot be a module name (spec 4.7).
pub const INVALID_ROOT_NAME: &str = "E2305";
/// The dependencies of `Pome.toml` cannot be used: no lock, or not in the cache for the language
/// server (spec 4.7).
pub const DEPENDENCIES_NOT_READY: &str = "E2306";
/// A dependency whose `[pome].targets` does not include the build target (spec 4.7).
pub const UNSUPPORTED_TARGET: &str = "E2307";
/// A dependency written for an older minor version of the compiler (`[pome].emela`, spec 4.7).
pub const OLDER_COMPILER_DEPENDENCY: &str = "W2301";
/// A Pome in `[external] allow` that is not depended on (spec 4.7).
pub const UNUSED_EXTERNAL_ALLOW: &str = "W2302";
/// `[pome].license` is not a string or not an SPDX license expression (spec `[pome.manifest]`).
pub const LICENSE_NOT_AN_EXPRESSION: &str = "W2303";
/// An experimental feature named in `[experimental]` or by a flag `--experimental-<name>` is
/// unknown, stable or removed (spec `[intro.experimental]`).
pub const UNKNOWN_EXPERIMENTAL_FEATURE: &str = "W2304";
/// A value of `[experimental]` is not a boolean (spec `[pome.manifest]`).
pub const EXPERIMENTAL_NOT_A_BOOLEAN: &str = "W2305";

// E30xx: types in general
pub const TYPE_MISMATCH: &str = "E3001";
pub const INFINITE_TYPE: &str = "E3002";
pub const NOT_NUMERIC: &str = "E3003";
pub const INT_LITERAL_OUT_OF_RANGE: &str = "E3004";
pub const FIELD_ON_UNKNOWN_TYPE: &str = "E3005";
pub const MISSING_PARAM_TYPE: &str = "E3006";
pub const SET_ARG_MISMATCH: &str = "E3007";
pub const SET_AS_TYPE: &str = "E3008";
/// `[dyn.error]`: the entry's `main` returns a type other than `()`, `ExitCode` and Never.
pub const MAIN_VALUE_DISCARDED: &str = "W3001";
/// `[fn.decl]`: a type parameter of a fn does not appear in the type of the fn.
pub const UNUSED_TYPE_PARAM: &str = "W3002";

// E31xx: calls and patterns
/// The wrong number of type arguments, arguments, fields of a constructor pattern, or
/// parameters of an fn in an impl or an operation of a handler.
pub const WRONG_COUNT: &str = "E3101";
pub const NON_EXHAUSTIVE_MATCH: &str = "E3102";
pub const UNKNOWN_LABEL: &str = "E3103";
pub const DUPLICATE_ARG: &str = "E3104";
pub const MISSING_PATTERN_FIELDS: &str = "E3105";
pub const NO_FIELD: &str = "E3106";
pub const NO_FIELDS: &str = "E3107";
pub const REFUTABLE_BINDING: &str = "E3108";
pub const FN_NAMED_LIKE_FIELD: &str = "E3109";
pub const AMBIGUOUS_METHOD: &str = "E3110";
/// A call to the name of a loop expression that is not in tail position of its body (`[fn.loop]`).
pub const LOOP_CALL_NOT_TAIL: &str = "E3111";
/// The name of a loop expression written as a value (`[fn.loop]`).
pub const LOOP_AS_VALUE: &str = "E3112";
pub const UNREACHABLE_MATCH_ARM: &str = "W3101";

// E32xx: error types (E)
/// The E of a body or of a function value is not included in the declared or expected E
/// (spec 7.4, 9.4).
pub const UNDECLARED_ERRORS: &str = "E3201";
pub const NOT_AN_ERROR: &str = "E3202";
pub const INVALID_ESCAPE_ARM: &str = "E3203";
pub const UNRAISED_ERROR: &str = "W3201";

// E33xx: traits, impls and derive
pub const INCOMPLETE_IMPL: &str = "E3301";
pub const NOT_IMPLEMENTED: &str = "E3302";
/// Found by name resolution, which sees the names in the head.
pub const IMPL_HEAD: &str = "E3303";
pub const SUPERTRAIT_CYCLE: &str = "E3304";
pub const DUPLICATE_IMPL: &str = "E3305";
pub const ORPHAN_IMPL: &str = "E3306";
pub const AMBIGUOUS_SELF: &str = "E3307";
pub const OPAQUE_DERIVE: &str = "E3308";
pub const DUPLICATE_KEY: &str = "E3309";
pub const MISPLACED_KEY_TAG: &str = "E3310";
pub const OBJECT_KEY_FORM: &str = "E3311";
pub const AUTO_TRAIT_IMPL: &str = "E3312";
pub const UNUSED_DERIVE: &str = "W3301";

// E40xx: declarations of effects and handlers
pub const MISSING_HANDLER_OPS: &str = "E4001";
pub const DUPLICATE_HANDLER_OP: &str = "E4002";
/// A handler for something a handler cannot implement: a host effect, External, a built-in
/// effect, or a label (`[effect.host]`, spec 8.9).
pub const UNIMPLEMENTABLE_BY_HANDLER: &str = "E4003";
pub const SUSPEND_MISMATCH: &str = "E4004";
pub const HANDLER_CTOR_OUTSIDE_INIT: &str = "E4005";
pub const MISSING_INIT: &str = "E4006";
pub const RELEASE_FAILS: &str = "E4007";
pub const MISSING_EFFECT_TYPE_ARGS: &str = "E4008";
pub const EFFECT_PARAM_NOT_TYPE: &str = "E4009";
pub const EFFECT_WITHOUT_OPS: &str = "E4010";
pub const INNER_HANDLER_MISMATCH: &str = "E4011";
/// An effect, or the name of an operation or a handler member, repeated across the effects
/// after the `:` of a handler (`[effect.handler]`).
pub const REPEATED_IN_HANDLER_HEAD: &str = "E4012";
pub const UNUSED_HANDLER_PARAM: &str = "E4013";

// E41xx: with, layers and handler members
pub const HANDLER_CYCLE: &str = "E4101";
pub const DUPLICATE_LAYER_EFFECT: &str = "E4102";
pub const LAYER_CYCLE: &str = "E4103";
/// Arguments of an init missing or not expected (spec 8.4).
pub const INIT_ARGS_MISMATCH: &str = "E4104";
pub const MEMBER_ITEM_FORM: &str = "E4105";
pub const MEMBER_OUTSIDE_INIT: &str = "E4106";
pub const HANDLER_ARGS_UNKNOWN: &str = "E4107";

// E42xx: effect labels
pub const LABEL_BASE: &str = "E4201";
pub const NOT_A_LABEL: &str = "E4202";
pub const LABEL_MISMATCH: &str = "E4203";
pub const LABEL_PROVIDED_TWICE: &str = "E4204";
pub const UNUSED_LABEL_PROVIDER: &str = "W4201";

// E43xx: R, Immediate and suspension
/// The R of a body or of a function value is not included in the R that the declaration,
/// the context or the expected function type allows (spec 8.3, 9.3).
pub const UNDECLARED_EFFECTS: &str = "E4301";
pub const NOT_IMMEDIATE: &str = "E4302";
pub const SUSPENDING_FN_OP: &str = "E4303";
pub const IMMEDIATE_ON_NON_EFFECT: &str = "E4304";
pub const SUSPEND_AFTER_RESUME: &str = "E4305";

// E44xx: capabilities
pub const UNKNOWN_OPERATION: &str = "E4401";
pub const USE_HOST_EFFECT: &str = "E4402";
pub const MISSING_CAPABILITY_EFFECTS: &str = "E4403";
pub const EFFECT_ALIAS_MISUSE: &str = "E4404";
pub const CAPABILITY_LEAK: &str = "E4405";
pub const CAPABILITY_IN_OP_RESULT: &str = "E4406";
pub const BOUND_TYPE_OUTSIDE_HANDLER: &str = "E4407";
pub const CAPABILITY_ON_SET: &str = "E4408";
/// `[effect.use]`: `use E` written as an expression is deprecated.
pub const DEPRECATED_USE: &str = "W4401";

// E50xx: `@external`
pub const MISSING_EXTERNAL_TARGET: &str = "E5001";
pub const EXTERNAL_USES: &str = "E5002";
pub const CALLBACK_EFFECT: &str = "E5003";
pub const BORROWED_POSITION: &str = "E5004";

// E51xx: `@export` and `@entry`
pub const NOT_BOUNDARY_EXPORT: &str = "E5101";
pub const EXPORT_OUTSIDE_ENTRY: &str = "E5102";
pub const DUPLICATE_EXPORT: &str = "E5103";
pub const NOT_ENTRY_ADAPTER: &str = "E5104";
pub const ENTRY_MISMATCH: &str = "E5105";

// E52xx: restrictions of a build target
pub const CALLBACK_EXTERNAL: &str = "E5201";
pub const SUSPENDING_EXPORT: &str = "E5202";
pub const HTTP_SERVICE_ON_C: &str = "E5203";
pub const BOUNDARY_TYPE_ON_C: &str = "E5204";

// E90xx: input and output
pub const PATH_NOT_FOUND: &str = "E9001";
pub const CANNOT_READ: &str = "E9002";
pub const CANNOT_WRITE_OUTPUT: &str = "E9003";

// E91xx: external tools
/// An external tool (the JS host, cc, wasmtime) is not found, cannot be started, or is too
/// old (`[cmd.build]`, `[target.native.cc]`, `[target.wasm.run]`).
pub const TOOL_UNAVAILABLE: &str = "E9101";
pub const INVALID_JS_OUTPUT: &str = "E9102";
pub const CC_FAILED: &str = "E9103";
/// `EMELA_NODE` was read to choose the JS host (`[cmd.build]`).
pub const NODE_ENV_DEPRECATED: &str = "W9101";
