//! `xerj … | head` must not dump core.
//!
//! The Rust runtime installs `SIG_IGN` for SIGPIPE before `main`, so a write to
//! a pipe whose reader has left returns `EPIPE`, `println!` panics on it
//! ("failed printing to stdout"), and the release profile's `panic = "abort"`
//! turns that panic into a core dump — the reported `xerj autoindex map |
//! head -80` failure, which also swallowed the command's real exit status.
//! `main` restores `SIG_DFL` so the kernel terminates us on the signal instead.
//!
//! Both tests drive `--help`: it is the only long stdout-only path that needs
//! neither a running server nor a fixture, and it prints through the same
//! `println!` that panicked.

#![cfg(unix)]

use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, ExitStatus, Stdio};

fn assert_died_quietly_on_sigpipe(status: ExitStatus, stderr: &[u8]) {
    let stderr = String::from_utf8_lossy(stderr);
    assert!(
        !stderr.contains("panicked"),
        "a closed stdout must not panic; stderr was: {stderr}"
    );
    assert!(
        !status.core_dumped(),
        "a closed stdout must not dump core; status was {status:?}"
    );
    assert_eq!(
        status.signal(),
        Some(libc::SIGPIPE),
        "expected termination by SIGPIPE (shell status 141), got {status:?}; stderr: {stderr}"
    );
}

/// The reader is dropped *before* the child is spawned, so there is no window
/// in which the write could have succeeded and no timing assumption at all.
#[test]
fn stdout_pipe_without_a_reader_dies_on_sigpipe_instead_of_aborting() {
    let (reader, writer) = std::io::pipe().expect("create pipe");
    drop(reader);

    let child = Command::new(env!("CARGO_BIN_EXE_xerj"))
        .arg("--help")
        .stdout(Stdio::from(writer))
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn xerj --help");
    let out = child.wait_with_output().expect("wait for xerj --help");

    assert_died_quietly_on_sigpipe(out.status, &out.stderr);
}

/// The reported shape: a reader that consumes a prefix and then leaves, as
/// `head -80` does. Linux-only because it needs `F_SETPIPE_SZ` to shrink the
/// pipe below the help text — that is what guarantees the child is still
/// mid-write when the reader goes, rather than having already buffered
/// everything into a 64 KiB pipe and exited 0.
#[cfg(target_os = "linux")]
#[test]
fn reader_leaving_mid_stream_dies_on_sigpipe_instead_of_aborting() {
    const PEEK: usize = 8;

    let full = Command::new(env!("CARGO_BIN_EXE_xerj"))
        .arg("--help")
        .output()
        .expect("run xerj --help");
    let (mut reader, writer) = std::io::pipe().expect("create pipe");
    // Shrinking bottoms out at one page; the kernel reports what it did set.
    let capacity = unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_SETPIPE_SZ, 4096) };
    assert!(
        capacity > 0,
        "F_SETPIPE_SZ failed: {}",
        std::io::Error::last_os_error()
    );
    // Not a flake when it trips: the child must still owe bytes after we have
    // read PEEK of them, or it finishes cleanly and proves nothing.
    assert!(
        full.stdout.len() > capacity as usize + PEEK,
        "`xerj --help` is {} bytes, which no longer exceeds the {capacity}-byte pipe — \
         drive this test with a longer-output subcommand",
        full.stdout.len()
    );

    let child = Command::new(env!("CARGO_BIN_EXE_xerj"))
        .arg("--help")
        .stdout(Stdio::from(writer))
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn xerj --help");
    let mut prefix = [0u8; PEEK];
    reader
        .read_exact(&mut prefix)
        .expect("read the head of the stream");
    drop(reader);

    let out = child.wait_with_output().expect("wait for xerj --help");

    assert_died_quietly_on_sigpipe(out.status, &out.stderr);
}
