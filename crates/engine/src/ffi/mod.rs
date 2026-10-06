// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The only `unsafe` code of the engine (ROADMAP M2.31, M2.34; PLAN 2.7 "Windows swap"): two Win32
//! calls that `std` does not expose, `ReplaceFileW` (atomic replace that keeps the replaced file's
//! creation time, DACL and named streams, `Zone.Identifier` included) and `GetDiskFreeSpaceExW`
//! (the free-space preflight). Each is declared by hand, wrapped in a function whose arguments
//! are safe Rust types, and nothing else in the crate touches a raw pointer.
//!
//! The directory is named `ffi` because `cargo xtask ci-guards` allows `unsafe` only in `ffi/` and
//! `simd/` module directories; every `unsafe` block carries a `// SAFETY:` comment. The crate
//! itself is `deny(unsafe_code)`, and only this module opts out.

#![allow(unsafe_code)]

#[cfg(windows)]
pub mod windows;
