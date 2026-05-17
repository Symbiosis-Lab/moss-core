//! moss HTML/CSS contract surface.
//!
//! Federated sources for the moss design contract:
//! - [`tokens`]: W3C Design Tokens (CSS variables, defaults, groups, descriptions)
//!
//! Consumed by the codegen binary at `src-tauri/dev-bin/generate-contract-docs.rs`,
//! by `moss describe` (Phase 2), and by snapshot tests.
//!
//! Source 2 (component contracts) is added in Phase 0b.

pub mod tokens;
