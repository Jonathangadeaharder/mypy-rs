//! Output rendering: byte-identical to mypy's summary for the supported
//! result shapes (one error class, one file).

use crate::check::Diagnostic;

/// Print every diagnostic line then the summary mypy emits for a
/// one-file run. Returns the process exit code: 0 clean, 1 with errors.
/// Diagnostics are sorted by (line, col), as mypy does: the checker
/// pushes in traversal order, which can report a later body line before
/// an earlier def-line diagnostic.
pub fn render(diagnostics: &[Diagnostic], path: &str) -> i32 {
    let mut ordered: Vec<&Diagnostic> = diagnostics.iter().collect();
    ordered.sort_by_key(|diag| (diag.line, diag.col));
    for diag in &ordered {
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
