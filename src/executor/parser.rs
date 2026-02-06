use crate::error::Result;
use regex::Regex;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    ShellCommand {
        command: String,
    },
    CreateFile {
        path: String,
        content: String,
    },
    EditFile {
        path: String,
        content: String,
        description: String,
    },
    DeleteFile {
        path: String,
    },
    RunTests {
        command: String,
    },
}

pub struct AIParser {
    shell_regex: Regex,
    code_block_regex: Regex,
    file_op_regex: Regex,
}

impl AIParser {
    pub fn new() -> Result<Self> {
        Ok(Self {
            shell_regex: Regex::new(
                r"```(?:bash|sh|shell)?\n([\s\S]*?)```|(?:^|\n)(?:\$\s|>|run\s|execute\s)(.+)",
            )?,
            code_block_regex: Regex::new(r"```(\w+)?\n([\s\S]*?)```")?,
            file_op_regex: Regex::new(
                r#"(?:create|write|edit|modify|update|delete|remove)\s+(?:the\s+)?(?:file\s+)?[`"']?([^`"'\n]+)[`"']?"#,
            )?,
        })
    }

    pub fn parse_response(&self, response: &str) -> Vec<Action> {
        let mut actions = Vec::new();

        // Parse shell commands from code blocks
        for action in self.parse_shell_commands(response) {
            actions.push(action);
        }

        // Parse file operations
        for action in self.parse_file_operations(response) {
            actions.push(action);
        }

        actions
    }

    fn parse_shell_commands(&self, response: &str) -> Vec<Action> {
        let mut commands = Vec::new();
        let shell_patterns = [
            (r"```bash\n([\s\S]*?)```", "bash"),
            (r"```sh\n([\s\S]*?)```", "sh"),
            (r"```shell\n([\s\S]*?)```", "shell"),
            (r"(?:^|\n)\$\s(.+)(?:\n|$)", "inline"),
        ];

        for (pattern, _) in &shell_patterns {
            if let Ok(regex) = Regex::new(pattern) {
                for cap in regex.captures_iter(response) {
                    if let Some(cmd) = cap.get(1) {
                        let command = cmd.as_str().trim().to_string();
                        if !command.is_empty() && !command.contains("```") {
                            commands.push(Action::ShellCommand { command });
                        }
                    }
                }
            }
        }

        commands
    }

    fn parse_file_operations(&self, response: &str) -> Vec<Action> {
        let mut actions = Vec::new();

        // Parse file path mentions with code blocks
        let file_with_code = Regex::new(
            r#"(?:create|add|write)\s+(?:the\s+)?(?:file\s+)?[`"']?([^`"'\n]+)[`"']?.*?```(?:\w+)?\n([\s\S]*?)```"#,
        );

        if let Ok(regex) = file_with_code {
            for cap in regex.captures_iter(response) {
                let path = cap
                    .get(1)
                    .map(|m| m.as_str().trim().to_string())
                    .unwrap_or_default();
                let content = cap
                    .get(2)
                    .map(|m| m.as_str().trim().to_string())
                    .unwrap_or_default();

                if !path.is_empty() && !content.is_empty() {
                    actions.push(Action::CreateFile { path, content });
                }
            }
        }

        // Parse edit operations
        let edit_pattern = Regex::new(
            r#"(?:edit|modify|update)\s+(?:the\s+)?(?:file\s+)?[`"']?([^`"'\n]+)[`"']?.*?```(?:\w+)?\n([\s\S]*?)```"#,
        );

        if let Ok(regex) = edit_pattern {
            for cap in regex.captures_iter(response) {
                let path = cap
                    .get(1)
                    .map(|m| m.as_str().trim().to_string())
                    .unwrap_or_default();
                let content = cap
                    .get(2)
                    .map(|m| m.as_str().trim().to_string())
                    .unwrap_or_default();

                if !path.is_empty() && !content.is_empty() {
                    actions.push(Action::EditFile {
                        path,
                        content,
                        description: "AI suggested edit".to_string(),
                    });
                }
            }
        }

        // Parse delete operations
        let delete_pattern =
            Regex::new(r#"(?:delete|remove)\s+(?:the\s+)?(?:file\s+)?[`"']?([^`"'\n]+)[`"']?"#);

        if let Ok(regex) = delete_pattern {
            for cap in regex.captures_iter(response) {
                let path = cap
                    .get(1)
                    .map(|m| m.as_str().trim().to_string())
                    .unwrap_or_default();

                if !path.is_empty() {
                    actions.push(Action::DeleteFile { path });
                }
            }
        }

        actions
    }

    pub fn detect_test_commands(&self, response: &str) -> Vec<String> {
        let mut test_commands = Vec::new();
        let test_patterns = [
            r#"(?:run|execute)?\s*tests?\s*(?:with|using)?\s*[`"']?(npm test|yarn test|cargo test|pytest|python -m pytest|go test)[`"']?"#,
            r#"```\n?(npm test|yarn test|cargo test|pytest|python -m pytest|go test)```"#,
        ];

        for pattern in &test_patterns {
            if let Ok(regex) = Regex::new(pattern) {
                for cap in regex.captures_iter(response) {
                    if let Some(cmd) = cap.get(1) {
                        test_commands.push(cmd.as_str().to_string());
                    }
                }
            }
        }

        test_commands
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_shell_command() {
        let parser = AIParser::new().unwrap();
        let response = r#"Run this command:
```bash
npm install express
```"#;
        let actions = parser.parse_response(response);

        assert!(!actions.is_empty());
        assert!(matches!(actions[0], Action::ShellCommand { .. }));
    }

    #[test]
    fn test_parse_create_file() {
        let parser = AIParser::new().unwrap();
        let response = r#"Create file `src/main.js`:
```javascript
console.log('hello');
```"#;

        let actions = parser.parse_response(response);
        assert!(actions
            .iter()
            .any(|a| matches!(a, Action::CreateFile { path, .. } if path == "src/main.js")));
    }
}
