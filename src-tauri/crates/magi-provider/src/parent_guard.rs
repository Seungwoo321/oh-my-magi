use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use tokio::process::Command;

pub(crate) struct ParentLease {
    _writer: OwnedFd,
}

pub(crate) fn install(command: &mut Command) -> io::Result<ParentLease> {
    let mut descriptors = [-1; 2];
    if unsafe { libc::pipe(descriptors.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let reader = unsafe { OwnedFd::from_raw_fd(descriptors[0]) };
    let writer = unsafe { OwnedFd::from_raw_fd(descriptors[1]) };
    for descriptor in [reader.as_raw_fd(), writer.as_raw_fd()] {
        if unsafe { libc::fcntl(descriptor, libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
            return Err(io::Error::last_os_error());
        }
    }
    let read_descriptor = reader.as_raw_fd();
    let write_descriptor = writer.as_raw_fd();
    let descriptor_limit = unsafe { libc::getdtablesize() };
    // The guard forks before exec and only calls async-signal-safe libc operations.
    // Closing inherited IPC descriptors prevents it from keeping provider pipes alive.
    unsafe {
        command.pre_exec(move || {
            let guard = libc::fork();
            if guard == -1 {
                return Err(io::Error::last_os_error());
            }
            if guard == 0 {
                let provider_pid = libc::getppid();
                let group = libc::getpgrp();
                for descriptor in 0..descriptor_limit {
                    if descriptor != read_descriptor {
                        libc::close(descriptor);
                    }
                }
                loop {
                    let mut poll = libc::pollfd {
                        fd: read_descriptor,
                        events: libc::POLLIN | libc::POLLHUP,
                        revents: 0,
                    };
                    let ready = libc::poll(&mut poll, 1, 250);
                    if libc::getppid() != provider_pid {
                        libc::kill(-group, libc::SIGKILL);
                        libc::_exit(0);
                    }
                    if ready > 0 && poll.revents != 0 {
                        let mut byte = 0u8;
                        if libc::read(read_descriptor, (&mut byte as *mut u8).cast(), 1) <= 0 {
                            libc::kill(-group, libc::SIGKILL);
                            libc::_exit(0);
                        }
                    }
                }
            }
            libc::close(read_descriptor);
            libc::close(write_descriptor);
            // The parent-owned copy closes after spawn; the child only uses raw FDs.
            let _ = reader.as_raw_fd();
            Ok(())
        });
    }
    Ok(ParentLease { _writer: writer })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn parent_lease_loss_kills_owned_provider_group() {
        let mut command = Command::new("/bin/sleep");
        command.arg("30").process_group(0).kill_on_drop(true);
        let lease = install(&mut command).unwrap();
        let mut child = command.spawn().unwrap();
        drop(command);
        drop(lease);
        let status = tokio::time::timeout(std::time::Duration::from_secs(3), child.wait())
            .await
            .expect("the guard must terminate the owned group")
            .unwrap();
        assert!(!status.success());
    }
}
