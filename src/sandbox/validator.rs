use crate::error::Result;
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub struct ValidationResult {
    pub passed: bool,
    pub checks: Vec<ValidationCheck>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ValidationCheck {
    pub name: String,
    pub passed: bool,
    pub message: String,
}

pub struct Validator {
    enabled_checks: HashSet<String>,
}

impl Validator {
    pub fn new() -> Self {
        let mut enabled_checks = HashSet::new();
        // Only check exit code by default - stderr can contain warnings/info
        // that aren't errors (npm progress, compiler warnings, etc.)
        enabled_checks.insert("exit_code".to_string());
        // "no_errors" and "tests_pass" can be enabled explicitly when needed

        Self { enabled_checks }
    }

    pub fn validate(&self, result: &super::docker::DockerResult) -> ValidationResult {
        let mut checks = Vec::new();
        let mut warnings = Vec::new();

        // Check 1: Exit code
        let exit_check = ValidationCheck {
            name: "exit_code".to_string(),
            passed: result.exit_code == 0,
            message: if result.exit_code == 0 {
                "Command exited successfully".to_string()
            } else {
                format!("Command failed with exit code {}", result.exit_code)
            },
        };
        checks.push(exit_check);

        // Check 2: No errors in stderr
        let no_errors = !self.contains_error_keywords(&result.stderr);
        let error_check = ValidationCheck {
            name: "no_errors".to_string(),
            passed: no_errors,
            message: if no_errors {
                "No error indicators in stderr".to_string()
            } else {
                "Error keywords detected in stderr".to_string()
            },
        };
        checks.push(error_check);

        // Check 3: Check for test results if present
        let tests_pass = self.check_test_results(&result.stdout, &result.stderr);
        let test_check = ValidationCheck {
            name: "tests_pass".to_string(),
            passed: tests_pass,
            message: if tests_pass {
                "All tests passed (or no tests ran)".to_string()
            } else {
                "Test failures detected".to_string()
            },
        };
        checks.push(test_check);

        // Collect warnings
        if result.duration_ms > 60000 {
            warnings.push(format!(
                "Command took {} seconds - consider optimization",
                result.duration_ms / 1000
            ));
        }

        if result.stderr.len() > 10000 {
            warnings.push("Large stderr output - review for potential issues".to_string());
        }

        // Overall passed if all enabled checks passed
        let passed = checks
            .iter()
            .filter(|c| self.enabled_checks.contains(&c.name))
            .all(|c| c.passed);

        ValidationResult {
            passed,
            checks,
            warnings,
        }
    }

    fn contains_error_keywords(&self, stderr: &str) -> bool {
        let error_keywords = [
            "error:",
            "Error:",
            "ERROR:",
            "fatal:",
            "Fatal:",
            "FATAL:",
            "exception",
            "Exception",
            "EXCEPTION",
            "panic:",
            "Panic:",
            "PANIC:",
            "failed",
            "Failed",
            "FAILED",
            "undefined reference",
            "syntax error",
            "compilation failed",
        ];

        error_keywords.iter().any(|kw| stderr.contains(kw))
    }

    fn check_test_results(&self, stdout: &str, stderr: &str) -> bool {
        let combined = format!("{} {}", stdout, stderr);

        // Common test framework patterns
        let test_patterns = [
            // Jest
            ("Tests:", "failed"),
            // Pytest
            ("passed", "failed"),
            // Cargo test
            ("test result:", "FAILED"),
            // Go test
            ("PASS", "FAIL"),
            // Mocha/Jasmine
            ("passing", "failing"),
        ];

        for (pass_indicator, fail_indicator) in &test_patterns {
            if combined.contains(pass_indicator) || combined.contains(fail_indicator) {
                // If we see the fail indicator, tests failed
                if combined.contains(fail_indicator) {
                    return false;
                }
            }
        }

        true
    }

    pub fn enable_check(&mut self, check_name: &str) {
        self.enabled_checks.insert(check_name.to_string());
    }

    pub fn disable_check(&mut self, check_name: &str) {
        self.enabled_checks.remove(check_name);
    }

    pub fn validate_file_changes(
        &self,
        changes: &[super::hydration::FileChange],
    ) -> ValidationResult {
        let mut checks = Vec::new();
        let mut warnings = Vec::new();

        // Check for suspicious patterns
        let dangerous_patterns = [
            "rm -rf /",
            "dd if=/dev/zero",
            ":(){ :|:& };:", // Fork bomb
        ];

        for change in changes {
            for pattern in &dangerous_patterns {
                if change.content.contains(pattern) {
                    checks.push(ValidationCheck {
                        name: "dangerous_content".to_string(),
                        passed: false,
                        message: format!(
                            "Dangerous pattern '{}' detected in {}",
                            pattern,
                            change.path.display()
                        ),
                    });
                }
            }

            // Check file size
            if change.content.len() > 10_000_000 {
                // 10MB
                warnings.push(format!(
                    "File {} is very large ({} MB)",
                    change.path.display(),
                    change.content.len() / 1_000_000
                ));
            }
        }

        // If no dangerous checks were added, add a passed check
        if !checks.iter().any(|c| c.name == "dangerous_content") {
            checks.push(ValidationCheck {
                name: "dangerous_content".to_string(),
                passed: true,
                message: "No dangerous patterns detected".to_string(),
            });
        }

        ValidationResult {
            passed: checks.iter().all(|c| c.passed),
            checks,
            warnings,
        }
    }
}
