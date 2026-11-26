//! The bundled `Std.Tcp` (`[api.std.tcp]`) through the `emela test` path, against servers that
//! this file starts on 127.0.0.1: `emela test` cannot start a server itself. The test sources are
//! `tcp/system_tcp.emel`, whose placeholders are filled with the ports of the servers and the
//! certificate of the TLS server.
//!
//! The TLS server is `tcp/tls_server.mjs` on node, with a self-signed certificate of `localhost`
//! and 127.0.0.1 (`tcp/cert.pem`, valid for 100 years). A self-signed certificate does not pass
//! the roots that the host trusts, so the tests check that it fails there, and that it passes
//! when it is given as `Pem`.
//!
//! Only JS runs the operations so far: native and WASM implement the registry of `SystemTcp`
//! (its init and release), and their operations are E5001 (`c::tcp_operations_are_e0412_natively`).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use emela_driver::{
    CoreJs, Input, LowerTests, MemoryFiles, Output, Target, TestOptions, TestOutcome, test,
};

/// Starts a server on 127.0.0.1 that runs `serve` on each connection in a thread of its own.
fn server(serve: impl Fn(TcpStream) + Send + Sync + 'static) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let serve = Arc::new(serve);
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let serve = Arc::clone(&serve);
            thread::spawn(move || serve(stream));
        }
    });
    port
}

/// Echoes until the client closes.
fn echo(mut s: TcpStream) {
    let mut buf = [0; 4096];
    while let Ok(n) = s.read(&mut buf) {
        if n == 0 || s.write_all(&buf[..n]).is_err() {
            break;
        }
    }
}

/// Reads until the client closes.
fn drain(mut s: TcpStream) {
    let mut buf = [0; 4096];
    while s.read(&mut buf).is_ok_and(|n| n > 0) {}
}

/// A port on which nothing listens.
fn refused_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

/// Tells each later client `closed` once its first client has closed. The first client is the
/// first connection accepted, not the first thread to run: the threads of the two connections may
/// run in either order.
fn watch_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let first_closed = Arc::new(AtomicBool::new(false));
    thread::spawn(move || {
        let mut incoming = listener.incoming().flatten();
        let Some(first) = incoming.next() else { return };
        let closed = Arc::clone(&first_closed);
        thread::spawn(move || {
            drain(first);
            closed.store(true, Ordering::SeqCst);
        });
        for mut s in incoming {
            let closed = Arc::clone(&first_closed);
            thread::spawn(move || {
                // Answers `open` if the first client is still open after 10 seconds, so that the
                // test fails instead of waiting.
                let mut answer: &[u8] = b"open  ";
                for _ in 0..1000 {
                    if closed.load(Ordering::SeqCst) {
                        answer = b"closed";
                        break;
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                let _ = s.write_all(answer);
                drain(s);
            });
        }
    });
    port
}

/// The TLS echo server on node, and its port. It ends when the child's standard input closes.
fn tls_server() -> (Child, u16) {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/tcp/tls_server.mjs");
    let mut child = Command::new("node")
        .arg(script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("node starts");
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let port = line.trim().parse().expect("the TLS server prints its port");
    (child, port)
}

/// The source of `tcp/system_tcp.emel` with its placeholders filled in.
fn source(tls: u16) -> String {
    let hello = server(|mut s| {
        let _ = s.write_all(b"hello");
        let _ = s.shutdown(Shutdown::Write);
        drain(s);
    });
    let split = server(|mut s| {
        let _ = s.write_all(b"abc");
        thread::sleep(Duration::from_millis(50));
        let _ = s.write_all(b"def");
        let _ = s.shutdown(Shutdown::Write);
        drain(s);
    });
    let early = server(|mut s| {
        let mut byte = [0; 1];
        if s.read_exact(&mut byte).is_ok() {
            let _ = s.write_all(b"Sx");
        }
        drain(s);
    });
    let cert = include_str!("tcp/cert.pem").replace('\n', "\\n");
    include_str!("tcp/system_tcp.emel")
        .replace("__ECHO__", &server(echo).to_string())
        .replace("__HELLO__", &hello.to_string())
        .replace("__SPLIT__", &split.to_string())
        .replace("__EARLY__", &early.to_string())
        .replace("__WATCH__", &watch_server().to_string())
        .replace("__REFUSED__", &refused_port().to_string())
        .replace("__TLS__", &tls.to_string())
        .replace("__CERT__", &cert)
}

/// The outcome of each test of `main` on JS, by name.
fn outcomes(main: &str) -> Vec<(String, TestOutcome)> {
    let fs = MemoryFiles::new([("app/Pome.toml", ""), ("app/src/main.emel", main)]);
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    let out_dir = std::env::temp_dir().join(format!("emela-driver-tcp-{}", std::process::id()));
    let options = TestOptions {
        out_dir: out_dir.clone(),
        js_host: emela_driver::js_host_program(),
        filter: None,
        output: Output::Capture,
        reporter: emela_driver::Reporter::Plain,
        color: false,
    };
    let mut out = Vec::new();
    let mut frontend = LowerTests::default().with_target(Target::Js);
    let (analysis, report) = test(
        &fs,
        &input,
        &mut frontend,
        &mut CoreJs,
        &options,
        |_| {},
        &mut out,
    );
    let _ = std::fs::remove_dir_all(&out_dir);
    assert!(
        analysis.diagnostics.is_empty(),
        "{:?}",
        analysis.diagnostics
    );
    let report = report.unwrap_or_else(|| panic!("{}", String::from_utf8_lossy(&out)));
    report
        .results
        .into_iter()
        .map(|r| (r.name, r.outcome))
        .collect()
}

#[test]
fn system_tcp_on_js() {
    let (mut tls, tls_port) = tls_server();
    let results = outcomes(&source(tls_port));
    drop(tls.stdin.take());
    let _ = tls.wait();
    let defects = [
        (
            "defect_read_max",
            "Tcp.read was given max 0, which is less than 1",
        ),
        (
            "defect_read_exact_negative",
            "Tcp.read_exact was given n -1, which is negative",
        ),
        (
            "defect_outside_the_with",
            "a connection was used outside the with of the handler that opened it",
        ),
        (
            "defect_another_handler",
            "a connection was used outside the with of the handler that opened it",
        ),
        ("defect_two_reads", "two reads of a connection ran at once"),
        (
            "defect_upgrade_twice",
            "Tcp.upgrade_tls was given a connection that is already TLS",
        ),
        (
            "defect_upgrade_while_reading",
            "Tcp.upgrade_tls was given a connection that is being read or written",
        ),
    ];
    let wrong: Vec<_> = results
        .iter()
        .filter(
            |(name, outcome)| match defects.iter().find(|(d, _)| name.ends_with(d)) {
                Some((_, message)) => !matches!(outcome, TestOutcome::Defect(m) if m == message),
                None => !matches!(outcome, TestOutcome::Passed),
            },
        )
        .collect();
    assert_eq!(wrong, Vec::<&(String, TestOutcome)>::new());
    assert!(results.len() > defects.len());
}
