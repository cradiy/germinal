# Windows packaging

Build on x64 Windows with the `x86_64-pc-windows-msvc` Rust toolchain, Visual Studio
C++ Build Tools, Windows SDK, and Nushell. `just` is an optional command wrapper.

- ZIP: install 7-Zip and make `7z` available on PATH.
- Installer: install Inno Setup 6.3 or later. Standard installation paths are detected;
  alternatively set `ISCC` to the full path of `ISCC.exe`.

```text
just build-windows
just build-windows-zip
just build-windows-setup
```

Without `just`, or to repackage an existing build:

```text
nu scripts/build.nu windows
nu scripts/build.nu windows zip --skip-build
nu scripts/build.nu windows setup --output dist
```

The scripts use the optimized `product` profile and an explicit Windows target.
The executable is at `target/x86_64-pc-windows-msvc/product/germinal.exe` (under
Cargo's configured target directory when overridden). `--skip-build` uses that same path.

Packages, SHA-256 checksum files, and a build manifest are written to `dist/`.
The ZIP contains `Germinal/germinal.exe` and the license. The per-user installer
installs into `%LOCALAPPDATA%\Programs\Germinal`, adds a Start menu shortcut and
provides an uninstaller. It does not require administrator privileges or modify PATH.
Packages are unsigned and do not bundle runtime DLLs or a shell.
