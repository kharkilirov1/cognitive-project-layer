# Install

The easiest installation path is the prebuilt GitHub Release binaries.

## Windows PowerShell

```powershell
irm https://raw.githubusercontent.com/kharkilirov1/cognitive-project-layer/main/install.ps1 | iex
```

Default install location:

```text
%USERPROFILE%\.cpl\bin
```

Install a specific version:

```powershell
.\install.ps1 -Version v0.5.0
```

Install without modifying the user `PATH`:

```powershell
.\install.ps1 -NoPath
```

## Linux and macOS

```bash
curl -fsSL https://raw.githubusercontent.com/kharkilirov1/cognitive-project-layer/main/install.sh | sh
```

Default install location:

```text
$HOME/.local/bin
```

Install a specific version:

```bash
VERSION=v0.5.0 sh install.sh
```

Install into a custom directory:

```bash
CPL_INSTALL_DIR="$HOME/bin" sh install.sh
```

## Manual install

1. Open the latest release:
   <https://github.com/kharkilirov1/cognitive-project-layer/releases/latest>
2. Download the archive for your OS/architecture.
3. Extract `cpl` and `cpl-mcp`.
4. Put them on your `PATH`.

Verify:

```bash
cpl --version
cpl-mcp --version
cpl scan --root .
```

## Published assets

Current release assets:

- `linux-x86_64`
- `windows-x86_64`
- `macos-x86_64`
- `macos-aarch64`

Each release includes `SHA256SUMS`.
