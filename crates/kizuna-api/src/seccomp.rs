//! Seccomp policy for the extraction worker (ADR 0016). The filter is compiled with `seccompiler` (rust-vmm /
//! Firecracker) to classic BPF and handed to bubblewrap (`bwrap --add-seccomp-fd FD`, twice), which installs it right
//! before it executes the worker, after its own namespace and mount setup. The policy is an allowlist derived from
//! `strace -f` of the worker over the extraction corpus (`scripts/seccomp-profile.sh`): every other syscall returns
//! EPERM, including the explicit deny list below (kept for the runtime refusal test and the status output). Any
//! non-native ABI (e.g. i386 `int 0x80`) kills the process (seccompiler's architecture check).

use std::collections::BTreeMap;
use std::sync::OnceLock;

use seccompiler::{BpfProgram, SeccompAction, SeccompFilter, TargetArch};

/// High-value syscalls that must stay refused (never allowlisted; checked at runtime in the sandbox), by name and
/// number.
pub fn denied() -> Vec<(&'static str, i64)> {
    macro_rules! sys {
        ($($n:ident),* $(,)?) => { vec![$((stringify!($n), libc::$n as i64)),*] };
    }
    #[allow(unused_mut)]
    let mut v = sys![
        // network: the worker has no network namespace peers anyway; refuse socket creation outright
        SYS_socket,
        SYS_socketpair,
        SYS_connect,
        SYS_bind,
        SYS_listen,
        SYS_accept,
        SYS_accept4,
        // other processes' memory
        SYS_ptrace,
        SYS_process_vm_readv,
        SYS_process_vm_writev,
        // filesystem / namespace changes
        SYS_mount,
        SYS_umount2,
        SYS_pivot_root,
        SYS_chroot,
        SYS_move_mount,
        SYS_open_tree,
        SYS_fsopen,
        SYS_fsconfig,
        SYS_fsmount,
        SYS_fspick,
        SYS_mount_setattr,
        SYS_unshare,
        SYS_setns,
        SYS_open_by_handle_at,
        SYS_name_to_handle_at,
        SYS_fanotify_init,
        // kernel attack surface
        SYS_bpf,
        SYS_perf_event_open,
        SYS_userfaultfd,
        SYS_io_uring_setup,
        SYS_io_uring_enter,
        SYS_io_uring_register,
        SYS_keyctl,
        SYS_add_key,
        SYS_request_key,
        // host administration
        SYS_kexec_load,
        SYS_kexec_file_load,
        SYS_init_module,
        SYS_finit_module,
        SYS_delete_module,
        SYS_reboot,
        SYS_swapon,
        SYS_swapoff,
        SYS_acct,
        SYS_quotactl,
        SYS_syslog,
        SYS_settimeofday,
        SYS_clock_settime,
        SYS_clock_adjtime,
        SYS_adjtimex,
        SYS_sethostname,
        SYS_setdomainname,
        // added after the deep review (KZ-S1): other processes, memory policy, new mount/LSM APIs
        SYS_pidfd_open,
        SYS_pidfd_getfd,
        SYS_pidfd_send_signal,
        SYS_process_madvise,
        SYS_process_mrelease,
        SYS_kcmp,
        SYS_personality,
        SYS_vmsplice,
        SYS_move_pages,
        SYS_migrate_pages,
        SYS_mbind,
        SYS_set_mempolicy,
        SYS_set_mempolicy_home_node,
        SYS_quotactl_fd,
        SYS_memfd_secret,
        SYS_fanotify_mark,
    ];
    // Newer than the libc crate's tables; numbers ≥ 424 are identical on every architecture (unified table).
    v.extend([
        ("SYS_statmount", 457),
        ("SYS_listmount", 458),
        ("SYS_lsm_get_self_attr", 459),
        ("SYS_lsm_set_self_attr", 460),
        ("SYS_lsm_list_modules", 461),
        ("SYS_open_tree_attr", 467),
    ]);
    #[cfg(target_arch = "x86_64")]
    v.extend(sys![
        SYS_iopl,
        SYS_ioperm,
        SYS_modify_ldt,
        SYS_uselib,
        SYS_lookup_dcookie
    ]);
    v
}

/// Syscalls the worker may make: everything `scripts/seccomp-profile.sh` observed over the extraction corpus
/// (`reports/seccomp/profile-<arch>.json`) except the denied ones, plus a small margin of runtime calls a CPython /
/// glibc / tesseract process can make on paths the corpus did not hit (signal return, thread exit, allocator
/// trimming, timeouts and killing its own tesseract child inside the sandbox's PID namespace). `ioctl` and `clone`
/// are further restricted by argument filters in [`program`]; `clone3` is answered with ENOSYS by the second filter.
/// Everything else (including every syscall a future kernel adds and the x32 ABI) returns EPERM.
pub fn allowed() -> Vec<(&'static str, i64)> {
    macro_rules! sys {
        ($($n:ident),* $(,)?) => { vec![$((stringify!($n), libc::$n as i64)),*] };
    }
    let mut v = sys![
        // observed over the corpus (reports/seccomp/profile-x86_64.json), generic names
        SYS_read,
        SYS_write,
        SYS_pread64,
        SYS_lseek,
        SYS_openat,
        SYS_close,
        SYS_fstat,
        SYS_newfstatat,
        SYS_readlinkat,
        SYS_getdents64,
        SYS_fcntl,
        SYS_ioctl, // argument filter: terminal/fd queries only (IOCTLS)
        SYS_pipe2,
        SYS_mmap,
        SYS_mprotect,
        SYS_munmap,
        SYS_mremap,
        SYS_brk,
        SYS_futex,
        SYS_rt_sigaction,
        SYS_rt_sigprocmask,
        SYS_execve,
        SYS_wait4,
        SYS_exit_group,
        SYS_getpid,
        SYS_gettid,
        SYS_getuid,
        SYS_getrandom,
        SYS_prlimit64,
        SYS_sched_getaffinity,
        SYS_set_robust_list,
        SYS_set_tid_address,
        SYS_rseq,
        SYS_clock_nanosleep,
        SYS_epoll_create1,
        // margin: runtime paths the corpus did not hit
        SYS_rt_sigreturn,
        SYS_restart_syscall,
        SYS_sigaltstack,
        SYS_exit,
        SYS_clone,  // argument filter: no namespace flags
        SYS_clone3, // the second filter turns it into ENOSYS (glibc then uses clone)
        SYS_madvise,
        SYS_sched_yield,
        SYS_nanosleep,
        SYS_clock_gettime,
        SYS_clock_getres,
        SYS_gettimeofday,
        SYS_getppid,
        SYS_geteuid,
        SYS_getgid,
        SYS_getegid,
        SYS_uname,
        SYS_sysinfo,
        SYS_statx,
        SYS_fstatfs,
        SYS_getcwd,
        SYS_faccessat,
        SYS_faccessat2,
        SYS_dup,
        SYS_dup3,
        SYS_ppoll,
        SYS_pselect6,
        SYS_epoll_ctl,
        SYS_epoll_pwait,
        SYS_waitid,
        SYS_kill,   // only reaches processes in the sandbox's own PID namespace (timeout of tesseract)
        SYS_tgkill, // abort()/raise() on itself
    ];
    #[cfg(target_arch = "x86_64")]
    v.extend(sys![
        // observed, legacy x86-64 entry points
        SYS_arch_prctl,
        SYS_access,
        SYS_open,
        SYS_stat,
        SYS_lstat,
        SYS_readlink,
        SYS_poll,
        SYS_vfork,
        // margin
        SYS_pipe,
        SYS_dup2,
        SYS_select,
        SYS_epoll_wait,
        SYS_time,
    ]);
    v
}

/// `ioctl` requests the worker may issue (compared on the low 32 bits): terminal attribute queries (`isatty`,
/// observed: TCGETS2) and plain fd queries. Everything else, e.g. TIOCSTI or device ioctls, returns EPERM.
pub const IOCTLS: [(&str, u64); 6] = [
    ("TCGETS", 0x5401),
    ("TCGETS2", 0x802C_542A),
    ("FIONREAD", 0x541B),
    ("FIONBIO", 0x5421),
    ("FIOCLEX", 0x5451),
    ("FIONCLEX", 0x5450),
];

/// Allowlisted syscall names without the `SYS_` prefix (as strace prints them).
pub fn allowed_names() -> Vec<String> {
    allowed()
        .iter()
        .map(|(n, _)| n.trim_start_matches("SYS_").to_string())
        .collect()
}

fn arch() -> Result<TargetArch, String> {
    TargetArch::try_from(std::env::consts::ARCH).map_err(|e| e.to_string())
}

/// `clone` flags that create namespaces: refused by an argument filter (glibc thread/fork clones never set them).
pub const CLONE_NS_FLAGS: [u64; 8] = [
    libc::CLONE_NEWNS as u64,
    libc::CLONE_NEWCGROUP as u64,
    libc::CLONE_NEWUTS as u64,
    libc::CLONE_NEWIPC as u64,
    libc::CLONE_NEWUSER as u64,
    libc::CLONE_NEWPID as u64,
    libc::CLONE_NEWNET as u64,
    0x80, // CLONE_NEWTIME
];

/// Compiles the main policy for this host's architecture: default EPERM; the allowlist is allowed, `clone` only
/// without namespace flags and `ioctl` only with an [`IOCTLS`] request. The deny list never overlaps the allowlist
/// (unit test), so every denied syscall gets the default EPERM.
pub fn program() -> Result<BpfProgram, String> {
    use seccompiler::{SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompRule};
    let rule = |arg: u8, len: SeccompCmpArgLen, op: SeccompCmpOp, val: u64| {
        SeccompCondition::new(arg, len, op, val)
            .and_then(|c| SeccompRule::new(vec![c]))
            .map_err(|e| e.to_string())
    };
    let mut rules: BTreeMap<i64, Vec<SeccompRule>> = allowed().into_iter().map(|(_, n)| (n, vec![])).collect();
    let ns_mask = CLONE_NS_FLAGS.iter().fold(0, |m, f| m | f);
    rules.insert(
        libc::SYS_clone,
        vec![rule(0, SeccompCmpArgLen::Qword, SeccompCmpOp::MaskedEq(ns_mask), 0)?],
    );
    rules.insert(
        libc::SYS_ioctl,
        IOCTLS
            .iter()
            .map(|(_, req)| rule(1, SeccompCmpArgLen::Dword, SeccompCmpOp::Eq, *req))
            .collect::<Result<Vec<_>, _>>()?,
    );
    let filter = SeccompFilter::new(
        rules,
        SeccompAction::Errno(libc::EPERM as u32),
        SeccompAction::Allow,
        arch()?,
    )
    .map_err(|e| e.to_string())?;
    BpfProgram::try_from(filter).map_err(|e| e.to_string())
}

/// Second filter: `clone3` returns ENOSYS so glibc falls back to `clone`, where the namespace-flag filter applies
/// (clone3 passes flags in memory, which seccomp cannot inspect).
pub fn enosys_program() -> Result<BpfProgram, String> {
    let rules: BTreeMap<i64, Vec<seccompiler::SeccompRule>> = BTreeMap::from([(libc::SYS_clone3, vec![])]);
    let filter = SeccompFilter::new(
        rules,
        SeccompAction::Allow,
        SeccompAction::Errno(libc::ENOSYS as u32),
        arch()?,
    )
    .map_err(|e| e.to_string())?;
    BpfProgram::try_from(filter).map_err(|e| e.to_string())
}

fn to_bytes(p: &BpfProgram) -> Vec<u8> {
    let mut out = Vec::with_capacity(p.len() * 8);
    for i in p {
        out.extend_from_slice(&i.code.to_ne_bytes());
        out.push(i.jt);
        out.push(i.jf);
        out.extend_from_slice(&i.k.to_ne_bytes());
    }
    out
}

/// Both programs as raw `struct sock_filter[]` bytes, one per bubblewrap `--add-seccomp-fd`.
pub fn programs_bytes() -> Result<[&'static [u8]; 2], String> {
    static BYTES: OnceLock<Result<[Vec<u8>; 2], String>> = OnceLock::new();
    BYTES
        .get_or_init(|| Ok([to_bytes(&program()?), to_bytes(&enosys_program()?)]))
        .as_ref()
        .map(|[a, b]| [a.as_slice(), b.as_slice()])
        .map_err(Clone::clone)
}

/// The main program as raw bytes (kept for status output and tests).
pub fn program_bytes() -> Result<&'static [u8], String> {
    static BYTES: OnceLock<Result<Vec<u8>, String>> = OnceLock::new();
    BYTES
        .get_or_init(|| {
            let p = program()?;
            let mut out = Vec::with_capacity(p.len() * 8);
            for i in &p {
                out.extend_from_slice(&i.code.to_ne_bytes());
                out.push(i.jt);
                out.push(i.jf);
                out.extend_from_slice(&i.k.to_ne_bytes());
            }
            Ok(out)
        })
        .as_ref()
        .map(Vec::as_slice)
        .map_err(Clone::clone)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn policy_compiles_and_never_allows_a_denied_syscall() {
        let b = program_bytes().unwrap();
        assert_eq!(b.len() % 8, 0);
        assert!(
            b.len() / 8 > allowed().len(),
            "one jump per allowed syscall plus prologue"
        );
        let denied: BTreeSet<i64> = denied().iter().map(|(_, n)| *n).collect();
        let allowed: Vec<(&str, i64)> = allowed();
        let numbers: BTreeSet<i64> = allowed.iter().map(|(_, n)| *n).collect();
        assert_eq!(numbers.len(), allowed.len(), "no duplicates in the allowlist");
        for (n, nr) in &allowed {
            assert!(!denied.contains(nr), "{n} is both allowed and denied");
        }
        for n in [
            "SYS_socket",
            "SYS_connect",
            "SYS_ptrace",
            "SYS_mount",
            "SYS_unshare",
            "SYS_bpf",
        ] {
            assert!(super::denied().iter().any(|(d, _)| *d == n), "{n}");
        }
        for n in [
            "SYS_read",
            "SYS_write",
            "SYS_execve",
            "SYS_mmap",
            "SYS_openat",
            "SYS_exit_group",
            "SYS_rt_sigreturn",
        ] {
            assert!(allowed.iter().any(|(a, _)| *a == n), "{n}");
        }
    }

    /// The committed profile (scripts/seccomp-profile.sh) is covered: every syscall the worker made over the corpus is
    /// allowlisted unless it is on the deny list (the worker already runs with those refused, e.g. glibc's nscd
    /// `socket`/`connect` probe), and every observed ioctl request is allowed.
    #[test]
    #[cfg(target_arch = "x86_64")]
    fn committed_profile_is_covered_by_the_allowlist() {
        let raw = include_str!("../../../reports/seccomp/profile-x86_64.json");
        let profile: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert!(profile["failed"].as_array().unwrap().is_empty());
        assert!(profile["requests"].as_u64().unwrap() > 100, "corpus too small");
        let allowed: BTreeSet<String> = allowed_names().into_iter().collect();
        let denied: BTreeSet<String> = denied()
            .iter()
            .map(|(n, _)| n.trim_start_matches("SYS_").to_string())
            .collect();
        for name in profile["syscalls"].as_object().unwrap().keys() {
            assert!(
                allowed.contains(name) || denied.contains(name),
                "observed {name} is neither allowed nor denied"
            );
        }
        for req in profile["ioctl_requests"].as_array().unwrap() {
            assert!(IOCTLS.iter().any(|(n, _)| req == n), "ioctl {req} not allowed");
        }
    }
}
