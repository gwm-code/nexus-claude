//! Error Patterns Database - Pre-built patterns for common errors
//!
//! This module contains regex patterns and handlers for detecting and fixing
//! common errors across different programming languages and frameworks.

use regex::Regex;
use std::collections::HashMap;

/// A detected error pattern with context
#[derive(Debug, Clone)]
pub struct DetectedError {
    pub error_type: ErrorType,
    pub severity: ErrorSeverity,
    pub message: String,
    pub file_path: Option<String>,
    pub line_number: Option<usize>,
    pub column: Option<usize>,
    pub stack_trace: Option<String>,
    pub suggested_fix: Option<String>,
}

/// Types of errors we can detect
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ErrorType {
    // Rust errors
    RustCompilation,
    RustBorrowChecker,
    RustMissingImport,
    RustTypeMismatch,
    RustUnusedWarning,

    // JavaScript/TypeScript errors
    JsSyntax,
    JsReferenceError,
    JsTypeError,
    JsModuleNotFound,
    JsUndefinedVariable,
    TsTypeMismatch,
    TsMissingType,

    // Python errors
    PythonSyntax,
    PythonIndentation,
    PythonImportError,
    PythonAttributeError,
    PythonTypeError,
    PythonNameError,

    // Build/Package errors
    MissingDependency,
    VersionConflict,
    BuildFailure,
    TestFailure,
    LintError,

    // Runtime errors
    RuntimeException,
    MemoryError,

    // Generic
    Unknown,
}

/// Severity levels for errors
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ErrorSeverity {
    Info,
    Warning,
    Error,
    Critical,
}

/// Language-specific error handler
pub struct ErrorHandler {
    pub language: Language,
    pub patterns: Vec<ErrorPattern>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Language {
    Rust,
    JavaScript,
    TypeScript,
    Python,
    Go,
    Java,
    Unknown,
}

/// A single error pattern with regex and metadata
#[derive(Debug, Clone)]
pub struct ErrorPattern {
    pub name: String,
    pub error_type: ErrorType,
    pub severity: ErrorSeverity,
    pub regex: Regex,
    pub extract_file_path: bool,
    pub extract_line_number: bool,
    pub extract_stack_trace: bool,
    pub auto_fixable: bool,
    pub suggested_fix_template: Option<String>,
}

/// The patterns database containing all known error patterns
pub struct PatternsDatabase {
    handlers: HashMap<Language, ErrorHandler>,
    generic_patterns: Vec<ErrorPattern>,
}

/// Stack trace extraction patterns
#[derive(Debug, Clone)]
pub struct StackTracePatterns {
    /// Rust panic stack trace pattern
    pub rust_panic: Regex,
    /// Rust backtrace pattern
    pub rust_backtrace: Regex,
    /// JavaScript stack trace pattern
    pub js_stack: Regex,
    /// Python traceback pattern
    pub python_traceback: Regex,
    /// Generic stack trace line pattern
    pub generic_frame: Regex,
}

impl StackTracePatterns {
    pub fn new() -> Self {
        Self {
            // Rust panic pattern
            rust_panic: Regex::new(
                r"thread '[^']+' panicked at (?P<file>[^:]+):(?P<line>\d+):(?P<col>\d+):",
            )
            .unwrap(),
            // Rust backtrace pattern
            rust_backtrace: Regex::new(
                r"^\s*(?:\d+):\s+(?:0x[0-9a-f]+\s+)?(.+?)(?: at (.+):(\d+))?$",
            )
            .unwrap(),
            // JavaScript stack trace pattern
            js_stack: Regex::new(r"at\s+(?:(.+?)\s+\()?(?:(.+?):(\d+):(\d+))\)?").unwrap(),
            // Python traceback pattern - matches: File "path", line N, in function
            python_traceback: Regex::new(r##"File "([^"]+)", line (\d+), in (.+)"##).unwrap(),
            // Generic frame pattern (matches common stack trace formats)
            generic_frame: Regex::new(
                r##"(?:at|in|from)\s+['"]?([^'"(]+)['"]?(?:\s*\(\))?(?::\s*(\d+))?"##,
            )
            .unwrap(),
        }
    }
}

impl Default for StackTracePatterns {
    fn default() -> Self {
        Self::new()
    }
}

impl PatternsDatabase {
    pub fn new() -> Self {
        let mut db = Self {
            handlers: HashMap::new(),
            generic_patterns: Vec::new(),
        };

        db.register_rust_patterns();
        db.register_js_patterns();
        db.register_python_patterns();
        db.register_generic_patterns();

        db
    }

    /// Detect errors in log output
    pub fn detect_errors(
        &self,
        log_output: &str,
        language_hint: Option<Language>,
    ) -> Vec<DetectedError> {
        let mut errors = Vec::new();

        // Check language-specific patterns first
        if let Some(ref lang) = language_hint {
            if let Some(handler) = self.handlers.get(lang) {
                for pattern in &handler.patterns {
                    if let Some(detected) = self.match_pattern(pattern, log_output, lang.clone()) {
                        errors.push(detected);
                    }
                }
            }
        }

        // Check generic patterns
        for pattern in &self.generic_patterns {
            if let Some(detected) = self.match_pattern(pattern, log_output, Language::Unknown) {
                errors.push(detected);
            }
        }

        // Check all language handlers if no hint or for cross-language errors
        if language_hint.is_none() {
            for (lang, handler) in self.handlers.iter() {
                for pattern in &handler.patterns {
                    if let Some(detected) = self.match_pattern(pattern, log_output, lang.clone()) {
                        errors.push(detected);
                    }
                }
            }
        }

        errors
    }

    /// Extract stack trace from log output
    pub fn extract_stack_trace(&self, log_output: &str, language: Language) -> Option<String> {
        let patterns = StackTracePatterns::new();

        match language {
            Language::Rust => self.extract_rust_stack_trace(log_output, &patterns),
            Language::JavaScript | Language::TypeScript => {
                self.extract_js_stack_trace(log_output, &patterns)
            }
            Language::Python => self.extract_python_stack_trace(log_output, &patterns),
            _ => self.extract_generic_stack_trace(log_output, &patterns),
        }
    }

    /// Extract Rust panic and backtrace
    fn extract_rust_stack_trace(
        &self,
        log_output: &str,
        patterns: &StackTracePatterns,
    ) -> Option<String> {
        let mut stack_lines = Vec::new();
        let mut in_stack = false;

        for line in log_output.lines() {
            // Check for panic message
            if patterns.rust_panic.is_match(line) {
                in_stack = true;
                stack_lines.push(line.to_string());
                continue;
            }

            // Check for backtrace frames
            if in_stack {
                if patterns.rust_backtrace.is_match(line)
                    || line.contains("stack backtrace:")
                    || line.trim().starts_with("at ")
                    || line.trim().starts_with(|c: char| c.is_digit(10))
                {
                    stack_lines.push(line.to_string());
                } else if line.trim().is_empty() && stack_lines.len() > 5 {
                    // End of stack trace
                    break;
                }
            }
        }

        if stack_lines.len() > 1 {
            Some(stack_lines.join("\n"))
        } else {
            None
        }
    }

    /// Extract JavaScript/TypeScript stack trace
    fn extract_js_stack_trace(
        &self,
        log_output: &str,
        patterns: &StackTracePatterns,
    ) -> Option<String> {
        let mut stack_lines = Vec::new();
        let mut in_stack = false;

        for line in log_output.lines() {
            // Check for error message (usually starts with ErrorType:)
            if line.contains("Error:") || line.contains("exception:") {
                in_stack = true;
                stack_lines.push(line.to_string());
                continue;
            }

            // Check for stack frames
            if in_stack {
                if patterns.js_stack.is_match(line) || line.trim().starts_with("at ") {
                    stack_lines.push(line.to_string());
                } else if !line.trim().starts_with("at ") && !line.is_empty() {
                    // End of stack trace
                    if stack_lines.len() > 1 {
                        break;
                    }
                    in_stack = false;
                    stack_lines.clear();
                }
            }
        }

        if stack_lines.len() > 1 {
            Some(stack_lines.join("\n"))
        } else {
            None
        }
    }

    /// Extract Python traceback
    fn extract_python_stack_trace(
        &self,
        log_output: &str,
        patterns: &StackTracePatterns,
    ) -> Option<String> {
        let mut stack_lines = Vec::new();
        let mut in_traceback = false;

        for line in log_output.lines() {
            // Check for traceback start
            if line.starts_with("Traceback (most recent call last):") {
                in_traceback = true;
                stack_lines.push(line.to_string());
                continue;
            }

            // Check for traceback frames
            if in_traceback {
                if patterns.python_traceback.is_match(line)
                    || line.trim().starts_with("File \"")
                    || line.starts_with("  ")
                {
                    stack_lines.push(line.to_string());
                } else if line.contains(":") && !line.starts_with("  ") {
                    // This is likely the final exception line
                    stack_lines.push(line.to_string());
                    break;
                }
            }
        }

        if stack_lines.len() > 1 {
            Some(stack_lines.join("\n"))
        } else {
            None
        }
    }

    /// Extract generic stack trace (fallback)
    fn extract_generic_stack_trace(
        &self,
        log_output: &str,
        patterns: &StackTracePatterns,
    ) -> Option<String> {
        let mut stack_lines = Vec::new();
        let mut consecutive_frames = 0;

        for line in log_output.lines() {
            if patterns.generic_frame.is_match(line) {
                stack_lines.push(line.to_string());
                consecutive_frames += 1;

                if consecutive_frames >= 3 {
                    // We've found a stack trace
                }
            } else {
                if consecutive_frames >= 3 {
                    // End of stack trace
                    break;
                }
                consecutive_frames = 0;
                if stack_lines.len() < 3 {
                    stack_lines.clear();
                }
            }
        }

        if stack_lines.len() >= 3 {
            Some(stack_lines.join("\n"))
        } else {
            None
        }
    }

    /// Match a single pattern against log output
    fn match_pattern(
        &self,
        pattern: &ErrorPattern,
        log_output: &str,
        language: Language,
    ) -> Option<DetectedError> {
        if let Some(captures) = pattern.regex.captures(log_output) {
            let message = captures
                .get(0)
                .map(|m| m.as_str().to_string())
                .unwrap_or_else(|| "Unknown error".to_string());

            let file_path = if pattern.extract_file_path {
                captures
                    .name("file")
                    .or_else(|| captures.get(1))
                    .map(|m| m.as_str().to_string())
            } else {
                None
            };

            let line_number = if pattern.extract_line_number {
                captures
                    .name("line")
                    .or_else(|| captures.get(2))
                    .and_then(|m| m.as_str().parse::<usize>().ok())
            } else {
                None
            };

            let column = captures
                .name("col")
                .and_then(|m| m.as_str().parse::<usize>().ok());

            // Extract stack trace if enabled for this pattern
            let stack_trace = if pattern.extract_stack_trace {
                self.extract_stack_trace(log_output, language)
            } else {
                None
            };

            let suggested_fix = pattern
                .suggested_fix_template
                .as_ref()
                .map(|template| self.format_fix_template(template, &captures));

            Some(DetectedError {
                error_type: pattern.error_type.clone(),
                severity: pattern.severity.clone(),
                message,
                file_path,
                line_number,
                column,
                stack_trace,
                suggested_fix,
            })
        } else {
            None
        }
    }

    /// Format a fix template with capture groups
    fn format_fix_template(&self, template: &str, captures: &regex::Captures) -> String {
        let mut result = template.to_string();

        // Replace named captures
        for name in ["file", "line", "col", "symbol", "type"] {
            if let Some(cap) = captures.name(name) {
                result = result.replace(&format!("{{{}}}", name), cap.as_str());
            }
        }

        // Replace numbered captures
        for i in 1..=captures.len() {
            if let Some(cap) = captures.get(i) {
                result = result.replace(&format!("{{{}}}", i), cap.as_str());
            }
        }

        result
    }

    /// Register Rust-specific error patterns
    fn register_rust_patterns(&mut self) {
        let patterns = vec![
            // Compilation error: cannot find value
            ErrorPattern {
                name: "rust_undefined_variable".to_string(),
                error_type: ErrorType::RustCompilation,
                severity: ErrorSeverity::Error,
                regex: Regex::new(r"error\[E0425\]: cannot find value [`']?(?P<symbol>[^`']+)[`']? in this scope").unwrap(),
                extract_file_path: true,
                extract_line_number: true,
                extract_stack_trace: false,
                auto_fixable: true,
                suggested_fix_template: Some(
                    "Check if variable '{symbol}' is defined or imported. If it's a constant, ensure it's in scope.".to_string()
                ),
            },
            
            // Missing import
            ErrorPattern {
                name: "rust_missing_import".to_string(),
                error_type: ErrorType::RustMissingImport,
                severity: ErrorSeverity::Error,
                regex: Regex::new(r"error\[E0433\]: failed to resolve: use of undeclared (?:crate|module)[`]?(?P<symbol>[^`']*)[`']?").unwrap(),
                extract_file_path: true,
                extract_line_number: true,
                extract_stack_trace: false,
                auto_fixable: true,
                suggested_fix_template: Some(
                    "Add `use` statement for '{symbol}' or add the crate to dependencies in Cargo.toml".to_string()
                ),
            },
            
            // Type mismatch
            ErrorPattern {
                name: "rust_type_mismatch".to_string(),
                error_type: ErrorType::RustTypeMismatch,
                severity: ErrorSeverity::Error,
                regex: Regex::new(r"error\[E0308\]: mismatched types\s*-->\s*(?P<file>[^:]+):(?P<line>\d+):(?P<col>\d+)").unwrap(),
                extract_file_path: true,
                extract_line_number: true,
                extract_stack_trace: false,
                auto_fixable: false,
                suggested_fix_template: Some(
                    "Expected and found types don't match. Consider using `.into()`, `.as_ref()`, or explicit type conversion.".to_string()
                ),
            },
            
            // Borrow checker error
            ErrorPattern {
                name: "rust_borrow".to_string(),
                error_type: ErrorType::RustBorrowChecker,
                severity: ErrorSeverity::Error,
                regex: Regex::new(r"error\[E0[45]9\d\]: (cannot borrow|borrow of)").unwrap(),
                extract_file_path: true,
                extract_line_number: true,
                extract_stack_trace: true, // Enable for borrow checker errors to get context
                auto_fixable: false,
                suggested_fix_template: Some(
                    "Consider using `.clone()`, restructuring the code to reduce borrows, or using `RefCell`/`Mutex` for interior mutability.".to_string()
                ),
            },
            
            // Runtime panic (stack trace enabled)
            ErrorPattern {
                name: "rust_panic".to_string(),
                error_type: ErrorType::RuntimeException,
                severity: ErrorSeverity::Critical,
                regex: Regex::new(r"thread '[^']+' panicked at").unwrap(),
                extract_file_path: true,
                extract_line_number: true,
                extract_stack_trace: true,
                auto_fixable: false,
                suggested_fix_template: Some(
                    "A panic occurred at runtime. Check the stack trace to identify the source of the panic.".to_string()
                ),
            },
            
            // Unused variable warning
            ErrorPattern {
                name: "rust_unused_var".to_string(),
                error_type: ErrorType::RustUnusedWarning,
                severity: ErrorSeverity::Warning,
                regex: Regex::new(r"warning: unused variable: [`']?(?P<symbol>[^`']+)[`']?").unwrap(),
                extract_file_path: true,
                extract_line_number: true,
                extract_stack_trace: false,
                auto_fixable: true,
                suggested_fix_template: Some(
                    "Prefix variable with underscore: `_{symbol}` to silence warning, or remove if not needed.".to_string()
                ),
            },
            
            // Unused import warning
            ErrorPattern {
                name: "rust_unused_import".to_string(),
                error_type: ErrorType::RustUnusedWarning,
                severity: ErrorSeverity::Warning,
                regex: Regex::new(r"warning: unused import: [`']?(?P<symbol>[^`']+)[`']?").unwrap(),
                extract_file_path: true,
                extract_line_number: true,
                extract_stack_trace: false,
                auto_fixable: true,
                suggested_fix_template: Some(
                    "Remove unused import: `{symbol}`".to_string()
                ),
            },
            
            // Dead code warning
            ErrorPattern {
                name: "rust_dead_code".to_string(),
                error_type: ErrorType::RustUnusedWarning,
                severity: ErrorSeverity::Warning,
                regex: Regex::new(r"warning: (?:function|struct|enum|constant) [`']?(?P<symbol>[^`']+)[`']? is never used").unwrap(),
                extract_file_path: true,
                extract_line_number: true,
                extract_stack_trace: false,
                auto_fixable: true,
                suggested_fix_template: Some(
                    "Add `#[allow(dead_code)]` attribute or use/remove the item '{symbol}'".to_string()
                ),
            },
        ];

        self.handlers.insert(
            Language::Rust,
            ErrorHandler {
                language: Language::Rust,
                patterns,
            },
        );
    }

    /// Register JavaScript/TypeScript error patterns
    fn register_js_patterns(&mut self) {
        let patterns = vec![
            // Module not found - place first since it's most specific
            ErrorPattern {
                name: "js_module_not_found".to_string(),
                error_type: ErrorType::JsModuleNotFound,
                severity: ErrorSeverity::Error,
                regex: Regex::new(r##"Error: Cannot find module ['"](?P<symbol>[^'"]+)['"]"##).unwrap(),
                extract_file_path: true,
                extract_line_number: false,
                extract_stack_trace: false,
                auto_fixable: true,
                suggested_fix_template: Some(
                    "Install missing module: `npm install {symbol}` or check the import path is correct.".to_string()
                ),
            },
            
            // Syntax error
            ErrorPattern {
                name: "js_syntax_error".to_string(),
                error_type: ErrorType::JsSyntax,
                severity: ErrorSeverity::Error,
                regex: Regex::new(r"SyntaxError:\s*(.+)+" ).unwrap(),
                extract_file_path: false,
                extract_line_number: false,
                extract_stack_trace: true,
                auto_fixable: false,
                suggested_fix_template: Some("Fix syntax error: {1}".to_string()),
            },
            
            // Reference error (undefined variable)
            ErrorPattern {
                name: "js_reference_error".to_string(),
                error_type: ErrorType::JsReferenceError,
                severity: ErrorSeverity::Error,
                regex: Regex::new(r"ReferenceError:\s*(?P<symbol>\w+) is not defined").unwrap(),
                extract_file_path: false,
                extract_line_number: false,
                extract_stack_trace: true,
                auto_fixable: true,
                suggested_fix_template: Some(
                    "Define variable '{symbol}' or check for typos. If it should be global, use `window.{symbol}` or `global.{symbol}`.".to_string()
                ),
            },
            
            // Type error
            ErrorPattern {
                name: "js_type_error".to_string(),
                error_type: ErrorType::JsTypeError,
                severity: ErrorSeverity::Error,
                regex: Regex::new(r"TypeError:\s*(.+?)(?:\s*at\s|$)").unwrap(),
                extract_file_path: true,
                extract_line_number: true,
                extract_stack_trace: true,
                auto_fixable: false,
                suggested_fix_template: Some("Type error: {1}. Check that the value is the expected type before using.".to_string()),
            },
            
            // Runtime exception with stack trace - general catch-all (place last)
            ErrorPattern {
                name: "js_runtime_exception".to_string(),
                error_type: ErrorType::RuntimeException,
                severity: ErrorSeverity::Critical,
                regex: Regex::new(r"^(?:Error|Exception):\s*(.+)$").unwrap(),
                extract_file_path: true,
                extract_line_number: true,
                extract_stack_trace: true,
                auto_fixable: false,
                suggested_fix_template: Some("Runtime error with stack trace. Review the stack trace to locate the error source.".to_string()),
            },
            
            // Import error (ES modules)
            ErrorPattern {
                name: "js_import_error".to_string(),
                error_type: ErrorType::JsModuleNotFound,
                severity: ErrorSeverity::Error,
                regex: Regex::new(r##"SyntaxError: (?:Cannot use import statement outside a module|The requested module ['"](?P<symbol>[^'"]+)['"] does not provide an export named)"##).unwrap(),
                extract_file_path: true,
                extract_line_number: false,
                extract_stack_trace: false,
                auto_fixable: true,
                suggested_fix_template: Some(
                    r##"For ES modules, add "type": "module" to package.json or use .mjs extension. Check that the module exports the expected members."##.to_string()
                ),
            },
            
            // TypeScript type mismatch
            ErrorPattern {
                name: "ts_type_mismatch".to_string(),
                error_type: ErrorType::TsTypeMismatch,
                severity: ErrorSeverity::Error,
                regex: Regex::new(r##"error TS2\d+:\s*Type ['"](?P<type>[^'"]+)['"] is not assignable to type ['"]([^'"]+)['"]"##).unwrap(),
                extract_file_path: true,
                extract_line_number: true,
                extract_stack_trace: false,
                auto_fixable: false,
                suggested_fix_template: Some(
                    "Type mismatch: '{type}' doesn't match. Consider type assertion, type narrowing, or updating type definitions.".to_string()
                ),
            },
            
            // TypeScript missing property
            ErrorPattern {
                name: "ts_missing_property".to_string(),
                error_type: ErrorType::TsTypeMismatch,
                severity: ErrorSeverity::Error,
                regex: Regex::new(r##"error TS2339:\s*Property ['"](?P<symbol>[^'"]+)['"] does not exist on type"##).unwrap(),
                extract_file_path: true,
                extract_line_number: true,
                extract_stack_trace: false,
                auto_fixable: false,
                suggested_fix_template: Some(
                    "Property '{symbol}' doesn't exist on this type. Check for typos or extend the type definition.".to_string()
                ),
            },
        ];

        self.handlers.insert(
            Language::JavaScript,
            ErrorHandler {
                language: Language::JavaScript,
                patterns: patterns.clone(),
            },
        );

        self.handlers.insert(
            Language::TypeScript,
            ErrorHandler {
                language: Language::TypeScript,
                patterns,
            },
        );
    }

    /// Register Python error patterns
    fn register_python_patterns(&mut self) {
        let patterns = vec![
            // Syntax error
            ErrorPattern {
                name: "py_syntax".to_string(),
                error_type: ErrorType::PythonSyntax,
                severity: ErrorSeverity::Error,
                regex: Regex::new(r"SyntaxError:\s*(.+?)\n").unwrap(),
                extract_file_path: true,
                extract_line_number: true,
                extract_stack_trace: false,
                auto_fixable: false,
                suggested_fix_template: Some("Fix Python syntax: {1}".to_string()),
            },
            
            // Indentation error
            ErrorPattern {
                name: "py_indent".to_string(),
                error_type: ErrorType::PythonIndentation,
                severity: ErrorSeverity::Error,
                regex: Regex::new(r"IndentationError:\s*(.+?)\n").unwrap(),
                extract_file_path: true,
                extract_line_number: true,
                extract_stack_trace: false,
                auto_fixable: true,
                suggested_fix_template: Some(
                    "Fix indentation. Python requires consistent indentation (typically 4 spaces).".to_string()
                ),
            },
            
            // Import error - No module named
            ErrorPattern {
                name: "py_import_no_module".to_string(),
                error_type: ErrorType::PythonImportError,
                severity: ErrorSeverity::Error,
                regex: Regex::new(r##"ImportError:\s*No module named ['\"](?P<symbol>[^'\"]+)['\"]"##).unwrap(),
                extract_file_path: true,
                extract_line_number: false,
                extract_stack_trace: false,
                auto_fixable: true,
                suggested_fix_template: Some(
                    "Install missing package: `pip install {symbol}` or check the import statement.".to_string()
                ),
            },
            
            // Import error - Cannot import name
            ErrorPattern {
                name: "py_import_cannot_import".to_string(),
                error_type: ErrorType::PythonImportError,
                severity: ErrorSeverity::Error,
                regex: Regex::new(r##"ImportError:\s*cannot import name ['\"](?P<symbol>[^'\"]+)['\"]"##).unwrap(),
                extract_file_path: true,
                extract_line_number: false,
                extract_stack_trace: false,
                auto_fixable: true,
                suggested_fix_template: Some(
                    "Check import statement for '{symbol}'. The function or module may not exist or may have moved.".to_string()
                ),
            },
            
            // Module not found
            ErrorPattern {
                name: "py_module_not_found".to_string(),
                error_type: ErrorType::PythonImportError,
                severity: ErrorSeverity::Error,
                regex: Regex::new(r##"ModuleNotFoundError:\s*No module named ['"](?P<symbol>[^'"]+)['"]"##).unwrap(),
                extract_file_path: true,
                extract_line_number: false,
                extract_stack_trace: false,
                auto_fixable: true,
                suggested_fix_template: Some(
                    "Install missing module: `pip install {symbol}`".to_string()
                ),
            },
            
            // Name error (undefined variable)
            ErrorPattern {
                name: "py_name".to_string(),
                error_type: ErrorType::PythonNameError,
                severity: ErrorSeverity::Error,
                regex: Regex::new(r##"NameError:\s*name ['"](?P<symbol>[^'"]+)['"] is not defined"##).unwrap(),
                extract_file_path: true,
                extract_line_number: true,
                extract_stack_trace: false,
                auto_fixable: false,
                suggested_fix_template: Some(
                    "Define variable '{symbol}' before use or check for typos.".to_string()
                ),
            },
            
            // Attribute error
            ErrorPattern {
                name: "py_attribute".to_string(),
                error_type: ErrorType::PythonAttributeError,
                severity: ErrorSeverity::Error,
                regex: Regex::new(r##"AttributeError:\s*['"](?P<type>[^'"]+)['"] object has no attribute ['"](?P<symbol>[^'"]+)['"]"##).unwrap(),
                extract_file_path: true,
                extract_line_number: true,
                extract_stack_trace: false,
                auto_fixable: false,
                suggested_fix_template: Some(
                    "Object of type '{type}' doesn't have attribute '{symbol}'. Check for typos or wrong object type.".to_string()
                ),
            },
            
            // Type error
            ErrorPattern {
                name: "py_type".to_string(),
                error_type: ErrorType::PythonTypeError,
                severity: ErrorSeverity::Error,
                regex: Regex::new(r"TypeError:\s*(.+?)\n").unwrap(),
                extract_file_path: true,
                extract_line_number: true,
                extract_stack_trace: true,
                auto_fixable: false,
                suggested_fix_template: Some("Type error: {1}".to_string()),
            },
            
            // Traceback with exception
            ErrorPattern {
                name: "py_traceback".to_string(),
                error_type: ErrorType::RuntimeException,
                severity: ErrorSeverity::Error,
                regex: Regex::new(r"Traceback \(most recent call last\):").unwrap(),
                extract_file_path: true,
                extract_line_number: true,
                extract_stack_trace: true,
                auto_fixable: false,
                suggested_fix_template: Some("Python exception occurred. Review the traceback to identify the error source.".to_string()),
            },
        ];

        self.handlers.insert(
            Language::Python,
            ErrorHandler {
                language: Language::Python,
                patterns,
            },
        );
    }

    /// Register generic patterns that apply to any language
    fn register_generic_patterns(&mut self) {
        self.generic_patterns = vec![
            // Test failure
            ErrorPattern {
                name: "generic_test_failure".to_string(),
                error_type: ErrorType::TestFailure,
                severity: ErrorSeverity::Error,
                regex: Regex::new(r"(?i)test.*FAILED|failures?[=:]\s*(\d+)|assertion\s+failed").unwrap(),
                extract_file_path: true,
                extract_line_number: true,
                extract_stack_trace: true,
                auto_fixable: false,
                suggested_fix_template: Some("Test failed. Check test assertions and implementation.".to_string()),
            },
            
            // Lint error
            ErrorPattern {
                name: "generic_lint".to_string(),
                error_type: ErrorType::LintError,
                severity: ErrorSeverity::Warning,
                regex: Regex::new(r"(?i)lint (?:error|warning)|clippy|eslint|pylint|style error").unwrap(),
                extract_file_path: true,
                extract_line_number: true,
                extract_stack_trace: false,
                auto_fixable: true,
                suggested_fix_template: Some("Fix linting issue by following style guidelines.".to_string()),
            },
            
            // Build failure
            ErrorPattern {
                name: "generic_build_fail".to_string(),
                error_type: ErrorType::BuildFailure,
                severity: ErrorSeverity::Critical,
                regex: Regex::new(r"(?i)build (?:failed|error)|compilation (?:failed|error)|make: \*\*\*.*Error").unwrap(),
                extract_file_path: false,
                extract_line_number: false,
                extract_stack_trace: true,
                auto_fixable: false,
                suggested_fix_template: Some("Build failed. Check error messages above for details.".to_string()),
            },
            
            // Missing dependency
            ErrorPattern {
                name: "generic_missing_dep".to_string(),
                error_type: ErrorType::MissingDependency,
                severity: ErrorSeverity::Error,
                regex: Regex::new(r##"(?i)(?:dependency|package|crate|module) ['"]([^'"]+)['"] (?:not found|could not be resolved|is required but not installed)"##).unwrap(),
                extract_file_path: false,
                extract_line_number: false,
                extract_stack_trace: false,
                auto_fixable: true,
                suggested_fix_template: Some(
                    "Install missing dependency: {1}. Check package manager configuration.".to_string()
                ),
            },
            
            // Version conflict
            ErrorPattern {
                name: "generic_version_conflict".to_string(),
                error_type: ErrorType::VersionConflict,
                severity: ErrorSeverity::Error,
                regex: Regex::new(r"(?i)version conflict|incompatible version|requires.*but found|cannot satisfy|dependency resolution failed").unwrap(),
                extract_file_path: false,
                extract_line_number: false,
                extract_stack_trace: false,
                auto_fixable: false,
                suggested_fix_template: Some(
                    "Version conflict detected. Check dependency versions and update package configuration.".to_string()
                ),
            },
        ];
    }

    /// Detect language from file extension or content
    pub fn detect_language(file_path: &str) -> Language {
        let path = std::path::Path::new(file_path);
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        match ext.as_str() {
            "rs" => Language::Rust,
            "js" | "mjs" | "cjs" => Language::JavaScript,
            "ts" | "tsx" | "mts" | "cts" => Language::TypeScript,
            "py" | "pyw" => Language::Python,
            "go" => Language::Go,
            "java" => Language::Java,
            _ => Language::Unknown,
        }
    }

    /// Get suggested fixes for an error type
    pub fn get_suggested_fixes(&self, error_type: &ErrorType) -> Vec<String> {
        let mut fixes = Vec::new();

        for handler in self.handlers.values() {
            for pattern in &handler.patterns {
                if &pattern.error_type == error_type && pattern.suggested_fix_template.is_some() {
                    fixes.push(pattern.suggested_fix_template.as_ref().unwrap().clone());
                }
            }
        }

        for pattern in &self.generic_patterns {
            if &pattern.error_type == error_type && pattern.suggested_fix_template.is_some() {
                fixes.push(pattern.suggested_fix_template.as_ref().unwrap().clone());
            }
        }

        fixes
    }
}

impl Default for PatternsDatabase {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_rust_error() {
        let db = PatternsDatabase::new();

        let log = r#"error[E0425]: cannot find value `foo` in this scope
 --> src/main.rs:10:5
  |
10 |     println!("{}", foo);
  |                    ^^^ not found in this scope
"#;

        let errors = db.detect_errors(log, Some(Language::Rust));
        assert!(!errors.is_empty());
        assert_eq!(errors[0].error_type, ErrorType::RustCompilation);
        assert_eq!(errors[0].severity, ErrorSeverity::Error);
    }

    #[test]
    fn test_detect_js_module_not_found() {
        let db = PatternsDatabase::new();

        let log = r#"Error: Cannot find module 'express'
    at Function.Module._resolveFilename (internal/modules/cjs/loader.js:815:15)"#;

        let errors = db.detect_errors(log, Some(Language::JavaScript));
        assert!(!errors.is_empty());
        assert_eq!(errors[0].error_type, ErrorType::JsModuleNotFound);
    }

    #[test]
    fn test_detect_language_from_extension() {
        assert_eq!(PatternsDatabase::detect_language("file.rs"), Language::Rust);
        assert_eq!(
            PatternsDatabase::detect_language("file.ts"),
            Language::TypeScript
        );
        assert_eq!(
            PatternsDatabase::detect_language("file.py"),
            Language::Python
        );
        assert_eq!(
            PatternsDatabase::detect_language("file.txt"),
            Language::Unknown
        );
    }
}
