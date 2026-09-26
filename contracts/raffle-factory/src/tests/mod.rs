// This file is used by the orphan-module checker (scripts/check_orphan_modules.py)
// to verify all test submodules are reachable. The actual module declarations
// that the Rust compiler uses live in the inline `#[cfg(test)] mod tests { }`
// block in lib.rs; this file is never compiled. See issue #1052.

pub mod budget;
pub mod governance;
pub mod tests;
pub mod views;
