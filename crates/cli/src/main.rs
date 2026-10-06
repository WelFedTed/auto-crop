// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `auto-crop`: the headless command-line tool (ROADMAP M2.39-M2.49, M2.81). A thin shell over
//! the engine: it parses the command line, drives `auto-crop-engine`, and reports; detection,
//! rendering and every write of an original are the engine's. See `docs/cli.md`.
//!
//! No network: this binary links no HTTP, TLS or socket crate (`cargo xtask ci-guards`).

mod analyze;
mod args;
mod backups;
mod devpipeline;
mod doctor;
mod env;
mod exit;
mod glob;
mod inputs;
mod manifest;
mod pipeline;
mod pool;
mod process;
mod render;
mod report;
mod restore;
mod writer;

use args::Command;
use auto_crop_core::CancelToken;
use std::process::ExitCode;

fn execute(cli: args::Cli) -> u8 {
    let args::Cli { global, command } = cli;
    match command {
        Command::Help(topic) => {
            print!("{}", args::help(topic));
            return exit::OK;
        }
        Command::Version => {
            print!("{}", doctor::version_text());
            return exit::OK;
        }
        Command::DevPipeline(a) => return devpipeline::run(&a),
        _ => {}
    }
    let paths = match env::app_paths(&global) {
        Ok(p) => p,
        Err(m) => {
            eprintln!("error: {m}");
            return exit::PRECONDITION;
        }
    };
    // Ctrl+C and termination: stop taking new work, finish the file in progress (its commit is
    // never abandoned half way), report, exit 130.
    let cancel = CancelToken::new_batch();
    {
        let c = cancel.clone();
        let _ = ctrlc::set_handler(move || {
            if c.is_cancelled() {
                eprintln!("\nstill finishing the file in progress; it will stop right after it");
            } else {
                c.cancel();
                eprintln!("\ninterrupted: finishing the file in progress, then stopping");
            }
        });
    }
    let env = env::Env {
        paths,
        global,
        cancel,
    };
    match command {
        Command::Process(a) => process::run(*a, &env),
        Command::Analyze(a) => analyze::run(*a, &env),
        Command::Render(a) => render::run(*a, &env),
        Command::Restore(a) => restore::run(a, &env),
        Command::Backups(c) => backups::run(c, &env),
        Command::Doctor => doctor::run(&env),
        Command::Help(_) | Command::Version | Command::DevPipeline(_) => exit::INTERNAL,
    }
}

fn main() -> ExitCode {
    // A packaged build keeps the HEIC plugin folder beside the executable (no-op otherwise).
    auto_crop_engine::packaged::configure_heif_from_exe();
    // Lossy for the rare argument that is not valid Unicode, instead of a panic.
    let argv: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    if argv.is_empty() {
        eprint!("{}", args::help(None));
        return ExitCode::from(exit::USAGE);
    }
    let cli = match args::parse_cli(&argv) {
        Ok(c) => c,
        Err(u) => {
            eprintln!("error: {u}\n\ntry `auto-crop --help`");
            return ExitCode::from(exit::USAGE);
        }
    };
    match auto_crop_engine::run_isolated(std::panic::AssertUnwindSafe(|| execute(cli))) {
        Ok(code) => ExitCode::from(code),
        Err(msg) => {
            eprintln!("internal error: {msg}");
            ExitCode::from(exit::INTERNAL)
        }
    }
}

/// `docs/cli.md` cannot drift from the tool: every option, exit code and code the tool has is in it.
#[cfg(test)]
mod docs {
    use super::*;

    const DOC: &str = include_str!("../../../docs/cli.md");

    #[test]
    fn every_option_of_every_command_is_documented() {
        for (cmd, _) in args::COMMANDS {
            for f in args::flags_for(cmd).unwrap() {
                assert!(
                    DOC.contains(&format!("--{}", f.name)),
                    "`--{}` of `{cmd}` is not in docs/cli.md",
                    f.name
                );
                if let Some(c) = f.short {
                    assert!(
                        DOC.contains(&format!("-{c}")),
                        "`-{c}` of `{cmd}` is not in docs/cli.md"
                    );
                }
            }
            assert!(
                DOC.contains(&format!("auto-crop {cmd}")),
                "{cmd} has no usage line"
            );
        }
    }

    #[test]
    fn every_exit_code_and_registry_code_is_documented() {
        for (code, _) in exit::TABLE {
            assert!(
                DOC.contains(&format!("| {code} |")),
                "exit code {code} is not in the table of docs/cli.md"
            );
        }
        for c in manifest::CODES {
            assert!(DOC.contains(c), "{c} is not in docs/cli.md");
        }
    }

    #[test]
    fn the_schema_file_names_the_same_exit_codes() {
        let schema = include_str!("../../../docs/schema/run-manifest.v1.schema.json");
        for (code, _) in exit::TABLE {
            assert!(schema.contains(&code.to_string()), "{code}");
            assert!(schema.contains(exit::name(code)), "{}", exit::name(code));
        }
    }
}
