//! Seccomp policy for the extraction worker (ADR 0016). The filter is compiled with `seccompiler` (rust-vmm /
//! Firecracker) to classic BPF and handed to bubblewrap (`bwrap --seccomp FD`), which installs it right before it
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
    ];
    #[cfg(target_arch = "x86_64")]
    v.extend(sys![SYS_iopl, SYS_ioperm, SYS_modify_ldt, SYS_uselib]);
    v
}

fn arch() -> Result<TargetArch, String> {
    TargetArch::try_from(std::env::consts::ARCH).map_err(|e| e.to_string())
}

/// Compiles the policy for this host's architecture.
pub fn program() -> Result<BpfProgram, String> {
    let rules: BTreeMap<i64, Vec<seccompiler::SeccompRule>> = denied().into_iter().map(|(_, n)| (n, vec![])).collect();
    let filter = SeccompFilter::new(
        rules,
        SeccompAction::Allow,
        SeccompAction::Errno(libc::EPERM as u32),
        arch()?,
    )
    .map_err(|e| e.to_string())?;
    BpfProgram::try_from(filter).map_err(|e| e.to_string())
}

/// The program as the raw `struct sock_filter[]` bytes bubblewrap reads from its `--seccomp` fd.
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
