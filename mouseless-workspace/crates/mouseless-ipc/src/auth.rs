//! Socket peer authentication (audit G3, SPEC-ipc auth section).
//!
//! Filesystem permissions on `/run/mouseless` already restrict connects to the
//! `mouseless` group, but permissions can drift; the daemon therefore verifies
//! `SO_PEERCRED` on accept as defense in depth.
//!
//! The policy is a pure predicate so it is unit-testable without spawning
//! processes under foreign uids: a peer is allowed when it is the daemon user
//! itself or belongs to the configured group (primary or supplementary).

/// Credentials reported by `SO_PEERCRED`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerCred {
    /// Effective uid of the peer process.
    pub uid: u32,
    /// Effective gid of the peer process.
    pub gid: u32,
    /// Peer process id (used to read supplementary groups).
    pub pid: i32,
}

/// Daemon-side authorization policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthPolicy {
    /// uid the daemon runs as (always allowed).
    pub daemon_uid: u32,
    /// gid allowed to talk to the daemon (the `mouseless` group).
    pub allowed_gid: u32,
}

impl AuthPolicy {
    /// Build a policy for the current process and the `mouseless` group.
    pub fn for_current_process(allowed_gid: u32) -> Self {
        Self {
            daemon_uid: unsafe { libc::geteuid() },
            allowed_gid,
        }
    }
}

/// Pure authorization predicate.
///
/// `peer_groups` are the peer's supplementary groups (from `/proc/<pid>/status`);
/// pass an empty slice when they cannot be resolved — primary-gid and uid
/// checks still apply.
pub fn peer_allowed(cred: PeerCred, peer_groups: &[u32], policy: &AuthPolicy) -> bool {
    cred.uid == policy.daemon_uid
        || cred.gid == policy.allowed_gid
        || peer_groups.contains(&policy.allowed_gid)
}

/// Read `SO_PEERCRED` from a connected Unix stream (Linux).
#[cfg(target_os = "linux")]
pub fn peer_cred(stream: &std::os::unix::net::UnixStream) -> std::io::Result<PeerCred> {
    use std::os::fd::AsRawFd;

    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `cred`/`len` are valid for writes for the duration of the call and
    // the fd is owned by `stream`.
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut libc::ucred as *mut libc::c_void,
            &mut len,
        )
    };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(PeerCred {
        uid: cred.uid,
        gid: cred.gid,
        pid: cred.pid,
    })
}

/// Read a peer's supplementary groups from `/proc/<pid>/status`.
///
/// Returns an empty vector when the process already exited or `/proc` is not
/// readable; the caller still has the uid/primary-gid checks.
#[cfg(target_os = "linux")]
pub fn peer_groups(pid: i32) -> Vec<u32> {
    if pid <= 0 {
        return Vec::new();
    }
    let Ok(text) = std::fs::read_to_string(format!("/proc/{pid}/status")) else {
        return Vec::new();
    };
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Groups:") {
            return rest
                .split_whitespace()
                .filter_map(|g| g.parse().ok())
                .collect();
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    const POLICY: AuthPolicy = AuthPolicy {
        daemon_uid: 999,
        allowed_gid: 1001,
    };

    #[test]
    fn daemon_uid_allowed() {
        assert!(peer_allowed(
            PeerCred {
                uid: 999,
                gid: 1,
                pid: 1
            },
            &[],
            &POLICY
        ));
    }

    #[test]
    fn group_member_allowed_by_primary_or_supplementary() {
        assert!(peer_allowed(
            PeerCred {
                uid: 1000,
                gid: 1001,
                pid: 1
            },
            &[],
            &POLICY
        ));
        assert!(peer_allowed(
            PeerCred {
                uid: 1000,
                gid: 1000,
                pid: 1
            },
            &[27, 1001],
            &POLICY
        ));
    }

    #[test]
    fn stranger_refused() {
        assert!(!peer_allowed(
            PeerCred {
                uid: 1000,
                gid: 1000,
                pid: 1
            },
            &[27, 46],
            &POLICY
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn reads_own_peer_credentials_and_groups() {
        use std::os::unix::net::UnixStream;
        let (a, _b) = UnixStream::pair().unwrap();
        let cred = peer_cred(&a).unwrap();
        assert_eq!(cred.uid, unsafe { libc::geteuid() });
        assert_eq!(cred.pid, std::process::id() as i32);
        // Our own supplementary groups include our primary gid.
        let groups = peer_groups(std::process::id() as i32);
        assert!(groups.contains(&unsafe { libc::getegid() }));
    }
}
