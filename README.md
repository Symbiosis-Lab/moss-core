# moss-core

> Pure-Rust content engine that powers moss.

[![crates.io](https://img.shields.io/crates/v/moss-core.svg)](https://crates.io/crates/moss-core)
[![downloads](https://img.shields.io/crates/d/moss-core.svg)](https://crates.io/crates/moss-core)
[![docs.rs](https://docs.rs/moss-core/badge.svg)](https://docs.rs/moss-core)
[![license](https://img.shields.io/crates/l/moss-core)](./LICENSE)
[![MSRV](https://img.shields.io/crates/msrv/moss-core.svg)](./Cargo.toml)
[![status](https://img.shields.io/badge/status-experimental-orange)](#stability)
[![discussions](https://img.shields.io/github/discussions/Symbiosis-Lab/moss-core)](https://github.com/Symbiosis-Lab/moss-core/discussions)

> **Read-only mirror.** Source lives in the private moss monorepo. PRs cannot be merged here — see [CONTRIBUTING.md](CONTRIBUTING.md).

[moss](https://mosspub.com) is a desktop publishing app; this crate is its content engine. Pure Rust, no I/O — takes data in (strings, structs), returns data out (parsed AST, frontmatter, rendered HTML).

- [Quickstart](#quickstart)
- [Stability](#stability)
- [API docs on docs.rs](https://docs.rs/moss-core)
- [Discussions](https://github.com/Symbiosis-Lab/moss-core/discussions) · [Issues](https://github.com/Symbiosis-Lab/moss-core/issues) · [moss.pub](https://mosspub.com)

## Quickstart

```rust
use moss_core::frontmatter;

let raw = "---\ntitle: Hello\n---\n\nBody text";
let doc = frontmatter::parse(raw)?;
println!("{:?}", doc.frontmatter);
```

## Stability

This crate is 0.x. The API may change between minor versions until 1.0.
Breaking changes are documented in [CHANGELOG.md](./CHANGELOG.md).

## License

MIT — see [LICENSE](LICENSE).
