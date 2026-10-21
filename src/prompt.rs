//! Yes/no confirmation prompts shared by destructive commands.

use anyhow::{Context, Result};
use std::io::{self, Write};

/// Print `question` with a `[y/N]` suffix and read the answer from stdin.
/// Anything other than an explicit yes — including end of input — is no.
pub fn confirm(question: &str) -> Result<bool> {
    print!("{question} [y/N] ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .context("failed to read confirmation")?;
    Ok(is_yes(&input))
}

fn is_yes(input: &str) -> bool {
    let answer = input.trim();
    answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_explicit_yes_confirms() {
        for yes in ["y", "Y", "yes", "YES", "Yes", " y\n"] {
            assert!(is_yes(yes), "{yes:?} should confirm");
        }
        for no in ["", "\n", "n", "no", "yep", "sure", "y y"] {
            assert!(!is_yes(no), "{no:?} should not confirm");
        }
    }
}
