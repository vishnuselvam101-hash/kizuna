# ADR 0016: Seccomp filter for the extraction worker

Status: accepted (2026-11-05); amended 2026-11-06 (allowlist)

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

## Amendment 2026-11-05 (deep review KZ-S1/S2/S3)
- Deny list extended (pidfd_*, process_madvise/mrelease, kcmp, personality, vmsplice, move/migrate_pages, mbind,
  set_mempolicy*, quotactl_fd, memfd_secret, statmount/listmount, open_tree_attr, lsm_*, fanotify_mark,
  lookup_dcookie). `clone` with any `CLONE_NEW*` flag → EPERM (argument filter); `clone3` → ENOSYS via a second
  filter (`--add-seccomp-fd 3 --add-seccomp-fd 4`), so glibc falls back to the inspectable `clone`.
- Every denied syscall is invoked at runtime inside the real sandbox and must return EPERM
  (`seccomp_blocks_socket_ptrace_mount_in_sandbox`, table-driven from the Rust policy).
- Worker address space 1 GiB (`KIZUNA_WORKER_MEMORY_MB`) and `oom_score_adj=1000` (choom), so a hostile document
  cannot push the service cgroup into OOM-killing the server.
A full default-deny allowlist (strace-derived) remains the long-term goal; the residual default-allow surface is
documented here.

## Amendment 2026-11-06: allowlist (default EPERM)
- The policy is now an allowlist. `scripts/seccomp-profile.sh` records every distinct worker request the extraction
  test suite builds (`KIZUNA_REQUEST_DUMP`, `services/extraction/tests/conftest.py`; 289 requests on 2026-11-06:
  CSV, XLSX, PDF, scanned-PDF OCR via tesseract, images, DXF, hostile inputs), replays each through the real worker in
  the real sandbox without the filter under `strace -f` (syscalls from the sandboxed `execve` of prlimit on, including
  choom, Python and tesseract children), and writes `reports/seccomp/profile-<arch>.json`. 45 distinct syscalls were
  observed on x86-64; all are allowlisted except `socket`/`connect`, which glibc attempts for an nscd passwd lookup
  (no `/etc/nsswitch.conf` in the sandbox) and which stay denied as before.
- A small, listed margin of runtime calls the corpus did not hit is also allowed (`seccomp::allowed`: signal
  return, thread exit, `madvise`, timeouts, `kill`/`tgkill` inside the sandbox PID namespace, `statx`, `dup*`, ...).
  Every other syscall, including every syscall a future kernel adds and the x32 ABI, returns EPERM.
- `ioctl` is allowed only for TCGETS, TCGETS2, FIONREAD, FIONBIO, FIOCLEX, FIONCLEX (low 32 bits of the request);
  `clone` only without `CLONE_NEW*` flags; `clone3` → ENOSYS (second filter). bubblewrap now installs the clone3
  filter first and the allowlist last, because the allowlist refuses `prctl(PR_SET_SECCOMP)`.
- The deny list stays as the set that must never be allowlisted (unit test) and is still invoked at runtime in the
  sandbox (every entry → EPERM).

Tests: `seccomp_blocks_socket_ptrace_mount_in_sandbox` additionally checks that an unlisted harmless syscall
(`getcpu`) and an unlisted ioctl (`TIOCGWINSZ`) get EPERM, that FIONREAD and a Python thread still work, and the
control run without the filter; `committed_profile_is_covered_by_the_allowlist`;
`policy_compiles_and_never_allows_a_denied_syscall`; the ignored `seccomp_profile_corpus` (run by the script) replays
the corpus a second time with the filter on and requires the identical answer for every request (289/289 on
2026-11-06).

Consequences: a Python, pypdf, Pillow or tesseract upgrade can need a syscall outside the list; the symptom is a
worker error (EPERM inside the worker), never a wider sandbox. Re-run `scripts/seccomp-profile.sh` after such
upgrades and on any new architecture (only x86-64 has been profiled and tested;
aarch64 has not been built or profiled). The profile covers what the test corpus exercises, not every possible document.
