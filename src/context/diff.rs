/// Differential updates - only send changed content
use crate::error::Result;

/// Compute diff between old and new content
pub fn compute_diff(old_content: &str, new_content: &str) -> String {
    // Simple line-by-line diff for now
    // In production, use a proper diff library like `similar`
    let old_lines: Vec<&str> = old_content.lines().collect();
    let new_lines: Vec<&str> = new_content.lines().collect();

    let mut diff = String::from("Changes:\n");

    // Find added lines
    for (i, line) in new_lines.iter().enumerate() {
        if i >= old_lines.len() || old_lines[i] != *line {
            diff.push_str(&format!("+ {}\n", line));
        }
    }

    // Find removed lines
    for (i, line) in old_lines.iter().enumerate() {
        if i >= new_lines.len() || new_lines[i] != *line {
            diff.push_str(&format!("- {}\n", line));
        }
    }

    diff
}

/// Apply a diff to old content to get new content
pub fn apply_diff(old_content: &str, diff: &str) -> Result<String> {
    // TODO: Implement proper diff application
    // For now, just return the diff content as-is
    Ok(diff.to_string())
}
