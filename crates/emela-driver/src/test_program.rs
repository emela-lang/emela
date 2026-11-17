//! Writing the programs that the tests start.

use std::io::Write as _;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};

/// Writes `script` to `path` and gives it the permission bits `mode`, without opening `path` for
/// writing in this process.
///
/// A file that this process has open for writing is inherited by the child of a `fork` in another
/// thread until that child runs its program, and a file that is open for writing cannot be
/// started: the start fails with `ETXTBSY` ("Text file busy"). Tests start programs in parallel,
/// so a script written directly here would sometimes be busy when another test started it.
/// Writing it from a child process keeps the handle out of this process, so no `fork` here
/// inherits it. `chmod` opens nothing.
pub(crate) fn write(path: &Path, script: &str, mode: u32) {
    let mut child = Command::new("/bin/sh")
        .args(["-c", "cat > \"$1\"", "sh"])
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    assert!(
        child.wait().unwrap().success(),
        "cannot write `{}`",
        path.display()
    );
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}
