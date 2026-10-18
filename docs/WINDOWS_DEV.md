# Building and running on Windows

The CI gate for Windows is `cargo check --workspace --all-targets --locked`
(see `.github/workflows/ci.yml`, job `cross-platform`). It needs no linker and
no WebView2. Producing an actual `.exe` needs more.

## Prerequisites

1. **Rust 1.96.0 (msvc)** — `rustup toolchain install 1.96.0`. The `msvc` host
   needs a linker.
2. **MSVC build tools** — Visual Studio Build Tools 2022 with the
   `Microsoft.VisualStudio.Workload.VCTools` workload. Unattended install
   (elevated shell):

   ```bat
   setup.exe install --quiet --norestart ^
     --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended ^
     --productId Microsoft.VisualStudio.Product.BuildTools ^
     --channelId VisualStudio.17.Release --installPath C:\BuildTools
   ```

   `setup.exe` does **not** accept `--wait` (that is a bootstrapper-only flag;
   passing it exits 87). Wait for the process instead.
3. **WebView2 runtime** — preinstalled on current Windows; only needed to run
   the GUI, not to build.

## Memory: the failure mode that looks like a compiler bug

`cargo build` can die with

```
rustc-LLVM ERROR: out of memory
Allocation failed
```

or, on Windows, with exit code `0xC0000409` (`STATUS_STACK_BUFFER_OVERRUN`,
which is how Rust reports a failed stack commit). This is **not** a code
defect: rustc's peak allocation for this workspace (large generated crates
plus DWARF) exceeds the machine's commit limit.

Two things make it happen:

- **No page file.** With `AutomaticManagedPagefile = False` the commit limit
  equals physical RAM, so a 32 GB machine with ~26 GB already committed has
  ~5 GB of headroom — less than one rustc needs. Check with:

  ```powershell
  (Get-CimInstance Win32_ComputerSystem).AutomaticManagedPagefile
  (Get-CimInstance Win32_OperatingSystem).TotalVirtualMemorySize / 1MB
  ```

  Either enable System-managed paging (`SystemPropertiesAdvanced.exe` →
  Performance → Advanced → Virtual memory) or keep the build inside the
  headroom.
- **Parallel rustc processes.** Every job is a full compiler instance;
  `-j 20` on a 20-core machine multiplies the peak by twenty.

Working recipe on a constrained machine:

```bash
CARGO_PROFILE_DEV_DEBUG=0 cargo build -p arbitrageur-desktop -j 1
```

`CARGO_PROFILE_DEV_DEBUG=0` drops DWARF (the dominant allocation) for the whole
dev profile without touching the repository; `injective-std` already has
`debug = false` in the workspace profile because its debug info is worthless.

Use a separate target dir so MSVC artifacts never mix with the Linux
container's (`target/`):

```bash
CARGO_TARGET_DIR=target-win cargo build -p arbitrageur-desktop
```

## First run

State lives in `%USERPROFILE%\.arbitrageur\` (`state.sqlite3`,
`arbitrageur.lock`). The directory is created on first launch; if it cannot be
created the process refuses to start, because running two engines would
double-dispatch every order.

## Running the tests

```bash
CARGO_TARGET_DIR=target-win CARGO_PROFILE_DEV_DEBUG=0 cargo test -p arbitrageur-desktop -j 1
```

The `stronghold` rekey test alone runs seven Argon2 derivations (minutes); it
is not hung, see `docs/DIAGNOSIS_2026-10-17.md` §8.12.
