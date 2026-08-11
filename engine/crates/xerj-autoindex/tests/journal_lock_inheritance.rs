#![cfg(all(any(target_os = "linux", target_vendor = "apple"), not(miri)))]

use xerj_autoindex::state::Journal;

struct ForkedLockHolder {
    child: libc::pid_t,
    release_fd: libc::c_int,
    parent_read_fd: libc::c_int,
}

impl ForkedLockHolder {
    fn close_fd(fd: &mut libc::c_int) -> std::io::Result<()> {
        if *fd < 0 {
            return Ok(());
        }
        let owned_fd = *fd;
        *fd = -1;
        if unsafe { libc::close(owned_fd) } == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }

    fn close_parent_read(&mut self) -> std::io::Result<()> {
        Self::close_fd(&mut self.parent_read_fd)
    }

    fn release_and_reap(mut self) {
        let close_result = Self::close_fd(&mut self.release_fd);
        let wait_result = waitpid_retry(self.child);

        close_result.expect("close parent pipe writer");
        let status = wait_result.expect("wait for forked lock holder");
        assert!(
            libc::WIFEXITED(status),
            "forked lock holder did not exit normally: status={status}"
        );
        assert_eq!(
            libc::WEXITSTATUS(status),
            0,
            "forked lock holder reported a child-side syscall failure"
        );
        self.child = -1;
    }
}

impl Drop for ForkedLockHolder {
    fn drop(&mut self) {
        let _ = Self::close_fd(&mut self.release_fd);
        let _ = Self::close_fd(&mut self.parent_read_fd);
        if self.child > 0 {
            let _ = waitpid_retry(self.child);
            self.child = -1;
        }
    }
}

fn waitpid_retry(child: libc::pid_t) -> std::io::Result<libc::c_int> {
    loop {
        let mut status = 0;
        let waited = unsafe { libc::waitpid(child, &mut status, 0) };
        if waited == child {
            return Ok(status);
        }
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        return Err(error);
    }
}

#[cfg(target_os = "linux")]
unsafe fn child_errno() -> libc::c_int {
    extern "C" {
        fn __errno_location() -> *mut libc::c_int;
    }
    unsafe { *__errno_location() }
}

#[cfg(target_vendor = "apple")]
unsafe fn child_errno() -> libc::c_int {
    unsafe { *libc::__error() }
}

#[test]
fn journal_drop_unlocks_before_a_fork_inherited_descriptor_closes() {
    let state_dir = tempfile::tempdir().unwrap();
    let journal =
        Journal::open(state_dir.path(), "root", "http://engine", "ax", 300, false).unwrap();

    // The child blocks on this pipe while retaining every descriptor that
    // existed at fork. It performs only async-signal-safe libc calls: a fork
    // from the multithreaded test harness must not touch Rust runtime state in
    // the child before `_exit`.
    let mut pipe_fds = [-1; 2];
    if unsafe { libc::pipe(pipe_fds.as_mut_ptr()) } != 0 {
        panic!("pipe failed: {}", std::io::Error::last_os_error());
    }
    let child = unsafe { libc::fork() };
    if child < 0 {
        let error = std::io::Error::last_os_error();
        unsafe {
            libc::close(pipe_fds[0]);
            libc::close(pipe_fds[1]);
        }
        panic!("fork failed: {error}");
    }
    if child == 0 {
        unsafe {
            if libc::close(pipe_fds[1]) != 0 {
                libc::_exit(2);
            }
            let mut byte = 0_u8;
            loop {
                let read = libc::read(
                    pipe_fds[0],
                    (&mut byte as *mut u8).cast::<libc::c_void>(),
                    1,
                );
                if read == 0 {
                    break;
                }
                if read < 0 && child_errno() == libc::EINTR {
                    continue;
                }
                libc::_exit(3);
            }
            if libc::close(pipe_fds[0]) != 0 {
                libc::_exit(4);
            }
            libc::_exit(0);
        }
    }

    // Own child cleanup immediately. Any later panic closes the writer and
    // reaps the child instead of retaining the inherited lock or a zombie.
    let mut child = ForkedLockHolder {
        child,
        release_fd: pipe_fds[1],
        parent_read_fd: pipe_fds[0],
    };
    child
        .close_parent_read()
        .expect("close unused parent pipe reader");

    drop(journal);
    let reopened = Journal::open(state_dir.path(), "root", "http://engine", "ax", 300, false);

    // Release and reap before checking the result. The pre-fix EAGAIN remains
    // observable without leaving a child behind when the assertion fails.
    child.release_and_reap();
    let reopened = reopened
        .expect("dropping Journal must explicitly unlock before an inherited descriptor is closed");
    drop(reopened);
}
