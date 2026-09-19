//! `mypy-rs`: the standalone skeleton check driver (issue #91,
//! ADR-0008 Lane 2). One file in, mypy-format bytes out, zero Python.
//!
//! Exit codes: 0 clean, 1 errors found, 2 usage / input errors, 3
//! internal (kernel deferred where the corpus must be decidable).

mod check;
mod fixtures;
mod python_stubs;
mod render;
mod subset;

use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(message) => {
            eprintln!("mypy-rs: {message}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<ExitCode, String> {
    let mut args = std::env::args_os().skip(1);
    let Some(path) = args.next() else {
        return Err("usage: mypy-rs <file>".to_string());
    };
    if args.next().is_some() {
        return Err("usage: mypy-rs <file> (exactly one file)".to_string());
    }
    let path = path
        .into_string()
        .map_err(|_| "file paths must be valid UTF-8".to_string())?;
    let source = std::fs::read_to_string(&path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let ast = subset::parse_module(&source, &path)?;
    let fixtures = fixtures::Fixtures::load()?;
    let diagnostics = check::check_module(&ast, &fixtures, &path)?;
    let code = render::render(&diagnostics, &path);
    Ok(ExitCode::from(code as u8))
}
