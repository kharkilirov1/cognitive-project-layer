# Profiles

Cognitive Project Layer keeps framework-specific behavior as profiles layered on
top of the generic project scanner, symbol index, graph, retrieval, and MCP/CLI
interfaces.

Profiles are not separate products. They are small, testable rules that improve
context quality for a language or framework without weakening the universal core.

## Default profile

Applies to every repository.

- Ignored directories: `target/`, `node_modules/`, `dist/`, `build/`, `.git/`,
  `.cpl/`, caches, IDE folders, and common `.env` files.
- Entry candidates: common `main`, `lib`, `index`, app config files.
- Config detection: Cargo, npm, Python, Go, Java/Kotlin/Gradle, CMake, etc.
- Retrieval order: symbols/references, grep, vector search, graph expansion,
  skeleton context, confidence.

## ArkTS / HarmonyOS profile

This profile is intentionally kept as a public feature, not a private workflow.
It helps agents navigate HarmonyOS projects where generic code search often
misses app-specific entry points and generated/build folders.

Rules currently implemented:

- Source language detection for `.ets`.
- HarmonyOS ignored folders:
  - `.ohos/`
  - `.hvigor/`
  - `hvigor/`
  - `entry/build/`
  - `oh_modules/`
- Entry candidates:
  - `EntryAbility.ets`
  - `Index.ets`
  - `App.ets`
  - `entry/src/main/ets/entryability/EntryAbility.ets`
  - `entry/src/main/ets/pages/Index.ets`
  - `entry/src/main/module.json5`
  - `oh-package.json5`
  - `build-profile.json5`
- Config detection:
  - `oh-package.json5`
  - `build-profile.json5`
  - `hvigorfile.ts`
  - `module.json5`
- Regex fallback for ArkTS/HarmonyOS components and exports, including
  `@Component` + `export struct`.

## Adding a profile

When adding a new profile:

1. Keep profile-specific rules isolated in scanner/symbol/config classification.
2. Add tests for entry/config detection and symbol parsing.
3. Update this document and the roadmap.
4. Do not hard-code private project names or local paths.
