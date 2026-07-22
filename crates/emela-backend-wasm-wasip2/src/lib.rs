//! The WASI 0.2 component-model backend (`wasm-wasip2`, spec 0052).
//!
//! It reuses the core-module emission of `emela-backend-wasm` in
//! [`WasmTarget::Wasip2`] mode (a `run` entry, no preview1 surface, canonical-
//! ABI WASI imports) and wraps that core module into a WebAssembly **component**
//! with `wit-component`. Standard capabilities lower to their WASI 0.2
//! interfaces (`io.*`→`wasi:cli`/`wasi:io`; `socket.*`→`wasi:sockets` is added
//! next). The world imports only the interfaces the program uses, so the
//! generated component's imports are the audited leaf (spec 0025/0052). The
//! component runs under a WASI 0.2 runtime (e.g. `wasmtime run`), not wasmi.

use emela_backend_wasm::{WasmTarget, emit_module};
use emela_codegen::{
    Artifact, ArtifactKind, Backend, BackendError, BackendOptions, EmitMode, IrProgram, Result,
    Tier, insert_rc_ops, used_platform_fns,
};
use wit_component::{ComponentEncoder, StringEncoding, embed_component_metadata};
use wit_parser::Resolve;

/// The `wasm-wasip2` backend: emits a WASI 0.2 command component (spec 0052).
pub struct Wasip2Backend;

impl Backend for Wasip2Backend {
    fn name(&self) -> &str {
        "wasm-wasip2"
    }

    fn tier(&self) -> Tier {
        Tier::Tier1
    }

    fn compile(&self, ir: &IrProgram, options: &BackendOptions) -> Result<Artifact> {
        // ARC (spec 0048) on a private copy, like the core wasm backend.
        let mut ir = ir.clone();
        insert_rc_ops(&mut ir);
        let core_wat = emit_module(&ir, &options.platform_registry, WasmTarget::Wasip2)?;
        if options.mode == EmitMode::Text {
            // The component is a binary; `--emit text` shows the core module WAT.
            return Ok(Artifact::text(ArtifactKind::WasmText, core_wat));
        }
        let mut core = wat::parse_str(&core_wat).map_err(|err| {
            BackendError::with(
                "internal error: generated core WAT failed to assemble".to_string(),
                vec![err.to_string()],
            )
        })?;
        let bytes = encode_component(&mut core, &wit_world(&ir))?;
        Ok(Artifact {
            kind: ArtifactKind::WasmComponent,
            bytes,
        })
    }
}

/// Embeds the world metadata into `core` and encodes the component.
fn encode_component(core: &mut Vec<u8>, wit: &str) -> Result<Vec<u8>> {
    let mut resolve = Resolve::default();
    let pkg = resolve
        .push_str("emela.wit", wit)
        .map_err(|err| component_error("WIT resolve failed", err))?;
    let world = resolve
        .select_world(&[pkg], Some("command"))
        .map_err(|err| component_error("world selection failed", err))?;
    embed_component_metadata(core, &resolve, world, StringEncoding::UTF8)
        .map_err(|err| component_error("metadata embed failed", err))?;
    ComponentEncoder::default()
        .module(core)
        .map_err(|err| component_error("component module load failed", err))?
        .encode()
        .map_err(|err| component_error("component encode failed", err))
}

fn component_error(msg: &str, err: impl std::fmt::Display) -> BackendError {
    BackendError::with(format!("internal error: {msg}"), vec![format!("{err:#}")])
}

/// Builds the WIT for the program's leaf capabilities: an `emela:app` world that
/// exports `wasi:cli/run` and imports only the WASI interfaces the program uses.
/// The interface definitions are a minimal subset of WASI 0.2 — the ones the
/// backend lowers to — which a full runtime satisfies by subtyping.
fn wit_world(ir: &IrProgram) -> String {
    let used = used_platform_fns(ir);
    let has = |name: &str| used.iter().any(|n| n == name);
    let mut imports = String::new();
    if has("io.write_stdout") {
        imports.push_str("  import wasi:cli/stdout@0.2.0;\n");
    }
    if has("io.write_stderr") {
        imports.push_str("  import wasi:cli/stderr@0.2.0;\n");
    }
    if used.iter().any(|n| n.starts_with("socket.")) {
        imports.push_str(
            "  import wasi:sockets/instance-network@0.2.0;\n\
            \x20 import wasi:sockets/tcp-create-socket@0.2.0;\n\
            \x20 import wasi:sockets/tcp@0.2.0;\n\
            \x20 import wasi:io/poll@0.2.0;\n",
        );
    }
    format!(
        "package emela:app@0.2.0;\n\
         \n\
         world command {{\n\
         {imports}\
        \x20 export wasi:cli/run@0.2.0;\n\
         }}\n\
         \n\
         {WASI_DEFS}"
    )
}

/// A minimal subset of the WASI 0.2 interfaces the backend lowers to. Only the
/// functions/types actually used are declared; a real runtime provides the full
/// interfaces (a supertype), so import matching succeeds.
const WASI_DEFS: &str = "\
package wasi:io@0.2.0 {
  interface error {
    resource error;
  }
  interface poll {
    resource pollable {
      block: func();
    }
  }
  interface streams {
    use error.{error};
    use poll.{pollable};
    variant stream-error {
      last-operation-failed(error),
      closed,
    }
    resource input-stream {
      blocking-read: func(len: u64) -> result<list<u8>, stream-error>;
    }
    resource output-stream {
      blocking-write-and-flush: func(contents: list<u8>) -> result<_, stream-error>;
    }
  }
}

package wasi:sockets@0.2.0 {
  interface network {
    resource network;
    enum error-code {
      unknown,
      access-denied,
      not-supported,
      invalid-argument,
      out-of-memory,
      timeout,
      concurrency-conflict,
      not-in-progress,
      would-block,
      invalid-state,
      new-socket-limit,
      address-not-bindable,
      address-in-use,
      remote-unreachable,
      connection-refused,
      connection-reset,
      connection-aborted,
      datagram-too-large,
      name-unresolvable,
      temporary-resolver-failure,
      permanent-resolver-failure,
    }
    enum ip-address-family {
      ipv4,
      ipv6,
    }
    type ipv4-address = tuple<u8, u8, u8, u8>;
    type ipv6-address = tuple<u16, u16, u16, u16, u16, u16, u16, u16>;
    record ipv4-socket-address {
      port: u16,
      address: ipv4-address,
    }
    record ipv6-socket-address {
      port: u16,
      flow-info: u32,
      address: ipv6-address,
      scope-id: u32,
    }
    variant ip-socket-address {
      ipv4(ipv4-socket-address),
      ipv6(ipv6-socket-address),
    }
  }
  interface instance-network {
    use network.{network};
    instance-network: func() -> network;
  }
  interface tcp {
    use wasi:io/streams@0.2.0.{input-stream, output-stream};
    use wasi:io/poll@0.2.0.{pollable};
    use network.{network, error-code, ip-socket-address};
    resource tcp-socket {
      start-bind: func(network: borrow<network>, local-address: ip-socket-address) -> result<_, error-code>;
      finish-bind: func() -> result<_, error-code>;
      start-listen: func() -> result<_, error-code>;
      finish-listen: func() -> result<_, error-code>;
      accept: func() -> result<tuple<tcp-socket, input-stream, output-stream>, error-code>;
      subscribe: func() -> pollable;
    }
  }
  interface tcp-create-socket {
    use network.{network, error-code, ip-address-family};
    use tcp.{tcp-socket};
    create-tcp-socket: func(address-family: ip-address-family) -> result<tcp-socket, error-code>;
  }
}

package wasi:cli@0.2.0 {
  interface run {
    run: func() -> result;
  }
  interface stdout {
    use wasi:io/streams@0.2.0.{output-stream};
    get-stdout: func() -> output-stream;
  }
  interface stderr {
    use wasi:io/streams@0.2.0.{output-stream};
    get-stderr: func() -> output-stream;
  }
}
";
