# ADR 0016: Seccomp filter for the extraction worker

Status: accepted (2026-11-05)

## Context
ADR 0012 isolates the extraction worker with bubblewrap namespaces, `--cap-drop ALL`, prlimit and a read-only
minimal filesystem. The kernel syscall surface was still fully reachable (SECURITY.md residual risk).

## Decision
- bubblewrap installs a classic-BPF seccomp filter right before it executes the worker (`bwrap --seccomp 3`), after
  its own namespace/mount setup. The program is generated at runtime by `crates/kizuna-api/src/seccomp.rs` with
  `seccompiler` 0.5 (rust-vmm/Firecracker, Apache-2.0) and passed on fd 3 through a pipe (`command-fds`, no `unsafe`
  in our code). It is built once per process; if it cannot be built, the worker does not run (fail closed).
- Policy = deny list, default allow. Denied syscalls return `EPERM`:
  network (`socket`, `socketpair`, `connect`, `bind`, `listen`, `accept`, `accept4`); other processes
  (`ptrace`, `process_vm_readv/writev`); mounts and namespaces (`mount`, `umount2`, `pivot_root`, `chroot`, new mount
  API `fsopen/fsconfig/fsmount/fspick/move_mount/open_tree/mount_setattr`, `unshare`, `setns`,
  `open_by_handle_at`, `name_to_handle_at`, `fanotify_init`); kernel attack surface (`bpf`, `perf_event_open`,
  `userfaultfd`, `io_uring_*`, `keyctl`, `add_key`, `request_key`); host administration (`kexec_*`, module load/unload,
  `reboot`, `swapon/off`, `acct`, `quotactl`, `syslog`, clock setting, `sethostname`, `setdomainname`); x86-64 only
  `iopl`, `ioperm`, `modify_ldt`, `uselib`.
- Foreign ABIs: seccompiler's prologue kills the process when the syscall architecture is not the native one, so
  32-bit `int 0x80` calls (IA32 emulation is enabled on the pilot kernel) cannot bypass the list. The x32 ABI is
  disabled on the pilot kernel (`CONFIG_X86_X32_ABI` not set); re-check on other kernels.
- An allow list was rejected for now: CPython + pypdf + tesseract need a broad, version-dependent syscall set, and a
  missed syscall breaks extraction in production. The deny list removes the high-value primitives with no
  functional cost; an allow list can be derived later from `strace -f` of the extraction corpus.

## Tests
`extraction::tests::seccomp_blocks_socket_ptrace_mount_in_sandbox` runs a probe in the real sandbox: `socket`,
`ptrace` and `mount` fail with EPERM and `/proc/self/status` shows `Seccomp: 2`; a control run without the filter
shows `socket` and `ptrace` succeed there, so the refusal comes from the filter. (`mount` already fails without
CAP_SYS_ADMIN; the filter is a second layer.) The m3 extraction tests (CSV, XLSX, PDF, DXF, images) pass with the
filter. `seccomp::tests` checks the policy compiles and never denies basic syscalls.

## Consequences
Default-allow means unlisted dangerous syscalls (new kernel APIs) stay reachable until listed. Workers that need a
socket (none today) would need a policy change and a new ADR.
