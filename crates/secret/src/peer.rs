//! Peer identity: who is on the other end of the socket.
//!
//! On the device (Linux): SO_PEERCRED gives the peer's pid/uid, and
//! `/proc/<pid>/exe` gives the exe the kernel actually mapped — the
//! realpath allowlist in the policy is matched against that, so a shell
//! script `exec`ing through symlinks still lands on the real binary.
//!
//! Off-Linux (macOS host tests): there is no /proc; the identity falls
//! back to the `AGINX_SECRET_PEER_EXE` env var so the authz matrix stays
//! testable on the dev host. This is a dev/test hook only — production
//! is the phone (Linux), where the env path is not compiled in.

/// Who is asking. `exe` is the resolved executable path when the OS could
/// establish it; None means unidentified (and every op denies).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Peer {
    pub uid: u32,
    pub exe: Option<String>,
}

impl Peer {
    pub fn exe(&self) -> Option<&str> {
        self.exe.as_deref()
    }
}

#[cfg(target_os = "linux")]
pub fn peer_of(stream: &std::os::unix::net::UnixStream) -> Peer {
    use std::os::unix::io::AsRawFd;

    let mut cred = libc_ucred(0, 0, 0);
    let mut len = std::mem::size_of_val(&cred) as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut _ as *mut libc::c_void,
            &mut len,
        )
    };
    if rc != 0 {
        return Peer { uid: u32::MAX, exe: None };
    }
    let exe = std::fs::read_link(format!("/proc/{}/exe", cred.pid))
        .ok()
        .map(|p| p.to_string_lossy().into_owned());
    Peer { uid: cred.uid, exe }
}

// ucred is a bitfield-typed struct; name it through a shim so the cfg
// block above stays readable.
#[cfg(target_os = "linux")]
#[allow(non_snake_case)]
fn libc_ucred(pid: i32, uid: u32, gid: u32) -> libc::ucred {
    libc::ucred { pid, uid, gid }
}

#[cfg(not(target_os = "linux"))]
pub fn peer_of(_stream: &std::os::unix::net::UnixStream) -> Peer {
    let exe = std::env::var_os("AGINX_SECRET_PEER_EXE").map(|s| s.to_string_lossy().into_owned());
    Peer { uid: 0, exe }
}
