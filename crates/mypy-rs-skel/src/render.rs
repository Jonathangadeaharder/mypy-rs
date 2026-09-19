//! Output rendering: byte-identical to mypy's summary for the supported
//! result shapes (one error class, one file).

use crate::check::Diagnostic;

/// Print every diagnostic line then the summary mypy emits for a
/// one-file run. Returns the process exit code: 0 clean, 1 with errors.
pub fn render(diagnostics: &[Diagnostic], path: &str) -> i32 {
    for diag in diagnostics {
        println!("{path}:{}: {}", diag.line, diag.message);
    }
    if diagnostics.is_empty() {
        println!("Success: no issues found in 1 source file");
        0
    } else {
        let noun = if diagnostics.len() == 1 {
            "error"
        } else {
            "errors"
        };
        println!(
            "Found {n} {noun} in 1 file (checked 1 source file)",
            n = diagnostics.len()
        );
        1
    }
}
