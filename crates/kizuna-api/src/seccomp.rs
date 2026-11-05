//! Seccomp policy for the extraction worker (ADR 0016). The filter is compiled with `seccompiler` (rust-vmm /
//! Firecracker) to classic BPF and handed to bubblewrap (`bwrap --add-seccomp-fd FD`, twice), which installs it right before it
//! executes the worker, after its own namespace and mount setup. The policy is a deny list: syscalls the worker never
//! needs that would widen its reach (network, tracing other processes, mounts and namespaces, kernel keyrings, BPF,
//! io_uring, module and clock administration) return EPERM. Any non-native ABI (e.g. i386 `int 0x80`) kills the
//! process (seccompiler's architecture check).

use std::collections::BTreeMap;
use std::sync::OnceLock;

use seccompiler::{BpfProgram, SeccompAction, SeccompFilter, TargetArch};

/// Syscalls refused inside the sandbox, by name (for the ADR, status output and tests) and number.
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

/// Compiles the main policy for this host's architecture: the deny list (EPERM) plus `clone` with any namespace flag.
pub fn program() -> Result<BpfProgram, String> {
    use seccompiler::{SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompRule};
    let mut rules: BTreeMap<i64, Vec<SeccompRule>> = denied().into_iter().map(|(_, n)| (n, vec![])).collect();
    let clone_rules = CLONE_NS_FLAGS
        .iter()
        .map(|f| {
            SeccompCondition::new(0, SeccompCmpArgLen::Qword, SeccompCmpOp::MaskedEq(*f), *f)
                .and_then(|c| SeccompRule::new(vec![c]))
                .map_err(|e| e.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    rules.insert(libc::SYS_clone, clone_rules);
    let filter = SeccompFilter::new(
        rules,
        SeccompAction::Allow,
        SeccompAction::Errno(libc::EPERM as u32),
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

    #[test]
    fn policy_compiles_and_covers_required_syscalls() {
        let b = program_bytes().unwrap();
        assert_eq!(b.len() % 8, 0);
        assert!(
            b.len() / 8 > denied().len(),
            "one jump per denied syscall plus prologue"
        );
        let names: Vec<&str> = denied().iter().map(|(n, _)| *n).collect();
        for n in [
            "SYS_socket",
            "SYS_ptrace",
            "SYS_mount",
            "SYS_unshare",
            "SYS_bpf",
            "SYS_io_uring_setup",
        ] {
            assert!(names.contains(&n), "{n}");
        }
        // Never deny what the worker needs to run.
        for n in [
            "SYS_read",
            "SYS_write",
            "SYS_execve",
            "SYS_mmap",
            "SYS_openat",
            "SYS_exit_group",
        ] {
            assert!(!names.contains(&n), "{n}");
        }
    }
}
