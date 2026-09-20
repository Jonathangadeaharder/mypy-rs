//! `mypy-rs`: the standalone skeleton check driver (issue #91,
//! ADR-0008 Lane 2). One file in, mypy-format bytes out, zero Python.
//!
//! Exit codes: 0 clean, 1 errors found, 2 usage / input errors, 3
//! internal (unreadable fixtures, or a kernel deferral on a covered
//! file).

mod check;
mod fixtures;
mod model;
mod python_stubs;
mod render;
mod subset;

use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(Failure::Input(message)) => {
            eprintln!("mypy-rs: {message}");
            ExitCode::from(2)
        }
        Err(Failure::Internal(message)) => {
            eprintln!("mypy-rs: {message}");
            ExitCode::from(3)
        }
    }
}

/// Who broke the contract, which decides the exit code: the user's file
/// (usage, unreadable input, out-of-subset source) is `Input`; committed
/// fixtures or the kernel failing on a covered file is `Internal`.
enum Failure {
    Input(String),
    Internal(String),
}

impl From<check::CheckError> for Failure {
    fn from(error: check::CheckError) -> Self {
        match error {
            check::CheckError::Input(message) => Failure::Input(message),
            check::CheckError::Internal(message) => Failure::Internal(message),
        }
    }
}

fn run() -> Result<ExitCode, Failure> {
    let mut args = std::env::args_os().skip(1);
    let Some(path) = args.next() else {
        return Err(Failure::Input("usage: mypy-rs <file>".to_string()));
    };
    if args.next().is_some() {
        return Err(Failure::Input(
            "usage: mypy-rs <file> (exactly one file)".to_string(),
        ));
    }
    let path = path
        .into_string()
        .map_err(|_| Failure::Input("file paths must be valid UTF-8".to_string()))?;
    let source = std::fs::read_to_string(&path)
        .map_err(|e| Failure::Input(format!("cannot read {path}: {e}")))?;
    let path_ref = std::path::Path::new(&path);
    let module = path_ref
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| Failure::Input("the file must have a module name".to_string()))?;
    if !is_identifier(module) {
        return Err(Failure::Input(format!(
            "the file name `{module}` is not a valid module name"
        )));
    }
    let dir = match path_ref.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_string_lossy().into_owned(),
        _ => ".".to_string(),
    };
    let fixtures = fixtures::Fixtures::load().map_err(Failure::Internal)?;
    let mut driver = check::Driver::new(fixtures);
    let diagnostics = driver.check_main(&path, module, &dir, &source)?;
    let code = render::render(&diagnostics, &path);
    Ok(ExitCode::from(code as u8))
}

/// The main file's stem names the module. Only identifier-shaped stems
/// are accepted. mypy itself accepts any stem (verified on this build:
/// `class.py`, `a-b.py` and `1x.py` all check clean), so this is a
/// deliberate narrowing of the subset, not a mypy-matching rule.
fn is_identifier(stem: &str) -> bool {
    !stem.is_empty()
        && stem.bytes().enumerate().all(|(i, b)| {
            b == b'_'
                || if i == 0 {
                    b.is_ascii_alphabetic()
                } else {
                    b.is_ascii_alphanumeric()
                }
        })
}

#[cfg(test)]
mod tests {
    use super::is_identifier;

    #[test]
    fn module_name_rule() {
        for good in ["main", "_private", "a1", "x_2", "class"] {
            assert!(is_identifier(good), "{good} must be valid");
        }
        for bad in ["1main", "", "a-b", "café", "a b"] {
            assert!(!is_identifier(bad), "{bad} must reject");
        }
    }
}
