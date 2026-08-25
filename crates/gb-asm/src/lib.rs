// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! A small SM83 assembler, built so that PocketRust's demo cartridge can be
//! produced from source by this repository alone, with no external toolchain
//! anywhere in the path. See `src/main.rs` for the command-line front end.

pub mod asm;
pub mod expr;
pub mod opcodes;
