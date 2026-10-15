// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The operating-system glue with `unsafe` in it, one file per OS family. Everything here is a
//! thin, SAFETY-commented wrapper that exposes a safe API to the rest of the crate
//! (`cargo xtask ci-guards` allows `unsafe` only in a directory called `ffi`).

#[cfg(unix)]
pub mod unix;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(windows)]
pub mod windows;
