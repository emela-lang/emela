//! `emela-codegen` is the published core of the Emela compiler's back half:
//! the intermediate representation, the type-system types it references, and
//! the [`Backend`] interface that turns IR into target artifacts.
//!
//! The frontend (`emela`) lowers source to an [`IrProgram`]; backends such as
//! `emela-backend-js` and `emela-backend-wasm` implement [`Backend`] to turn
//! that IR into JavaScript or WebAssembly. Third parties can depend on this
//! crate alone to add a backend, in-process or as an external plugin.

mod backend;
mod error;
mod intrinsic;
mod ir;
mod ir_walk;
mod platform;
mod plugin;
mod registry;
mod text;
mod types;

pub use backend::{Artifact, ArtifactKind, Backend, BackendOptions, EmitMode, Tier};
pub use error::{BackendError, Result};
pub use intrinsic::{IntrinsicFn, intrinsic_interface, is_intrinsic, lookup as intrinsic_lookup};
pub use ir::{IrArm, IrCapture, IrExpr, IrFunction, IrParam, IrPattern, IrProgram, QuestionMode};
pub use ir_walk::{used_intrinsics, used_platform_fns, walk};
pub use platform::{PlatformFn, lookup as platform_lookup, platform_interface};
pub use plugin::{BackendDescriptor, ExternDescriptor, PluginRequest, PluginResponse};
pub use registry::BackendRegistry;
pub use text::emit_text;
pub use types::{BinaryOp, EffectRow, FunctionType, Type};
