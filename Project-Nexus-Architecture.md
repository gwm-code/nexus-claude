Project Nexus: Next-Generation AI CLI Architecture

Status: DRAFT | Date: Feb 2026

Target Models: Gemini 3 Pro (Context/Vision), Kimi k2.5 (Swarm Logic)

1. Executive Summary

Nexus is a "State-First" AI terminal interface designed to supersede the "Chat-First" paradigm of tools like OpenClaw. It shifts execution from linear, unverified command injection to a parallel, sandboxed, and verified engineering workflow.

Core Philosophy: "Trust, but Verify." The agent never runs code on the host machine without a successful "Shadow Run" in a sandbox.

2. High-Level System Design

graph TD
    User[User / Terminal] --> Core[Nexus Core (Rust/Go)]
    Core --> MCP[MCP Integration Layer]
    Core --> Memory[Context Engine (Gemini 3)]
    
    subgraph "Swarm Orchestrator (Kimi k2.5)"
        Architect[Architect Agent] --> DevA[Frontend Agent]
        Architect --> DevB[Backend Agent]
        Architect --> DevC[QA Agent]
    end
    
    Core --> Architect
    
    subgraph "The Safety Valve"
        Sandbox[Shadow Container / WASM]
        Validator[Test Runner]
    end
    
    DevA & DevB --> Sandbox
    Sandbox --> Validator
    Validator -- "Pass" --> Host[Host File System]
    Validator -- "Fail" --> Architect


3. Core Components

A. The Context Engine (The "Brain")

Primary Model: Gemini 3 Pro (Selected for 2M+ Token Context Window).

Mechanism: "Context Caching."

Instead of sending the full repo tree every request, Nexus maintains a "warm" cache of the project state.

Diff-Only Updates: Only file changes are sent to the model after the initial handshake.

Long-Term Memory: Integrated Mem0 vector store to remember user preferences (e.g., "I prefer arrow functions," "Never use any in TS") across different projects.

B. The Swarm Orchestrator (The "Hands")

Primary Model: Kimi k2.5 (Selected for Instruction Following & Agentic capabilities).

Framework: LangGraph.

Logic:

The Architect: Analyzes the high-level user request (e.g., "Refactor Auth") and breaks it into non-blocking tasks.

The Workers: Ephemeral sub-agents spawned for specific files or modules. They work in parallel, not sequentially.

The Merger: A dedicated routine that resolves Git conflicts between sub-agent outputs before presenting them to the user.

C. The Safety Layer ("Shadow Run")

Problem: OpenClaw executes commands blindly, risking environment corruption.

Solution: Ephemeral Sandboxing.

Command Interception: All shell commands (npm install, rm -rf) are intercepted.

Execution: Commands run in a lightweight Docker container or a dedicated WASM runtime mapped to the current directory.

Verification: If the exit code is 0 and critical tests pass, the changes are "hydrated" to the host machine. If 1, the agent self-corrects.

D. The Interface ("Headless" + PWA)

CLI Mode: Standard terminal input/output, but with rich TUI (Text User Interface) elements for diffs.

Headless Mode: Runs as a background daemon.

Local PWA: A localhost web server (e.g., localhost:8888) providing a visual dashboard.

Visual representation of the "Swarm" working.

"Big Red Button" to kill rogue agents.

Visual Diff Review before committing.

4. Integration Layer (MCP)

Nexus does not build custom integrations. It relies strictly on the Model Context Protocol (MCP).

Standard: Adheres to MCP 2026 specs.

Capabilities:

Allows the agent to read/write to local databases (SQLite, Postgres) via MCP servers.

Allows connection to external tools (GitHub Issues, Sentry, Slack) simply by adding their MCP config.

5. "Self-Healing" Watcher

A background process that hooks into the file system watcher (getting beyond simple chat).

Monitor: Watches standard output/error streams of the running dev server.

Detect: RegEx/AI detection of stack traces or build errors.

Investigate: Automatically reads the file mentioned in the stack trace.

Propose: Generates a fix and alerts the user: "I noticed a NullPointer in auth.ts. I have a fix ready. [Apply]?"

6. Tech Stack Recommendation

Component

Technology

Reasoning

Core Binary

Rust (or Go)

Memory safety, speed, zero-dependency distribution.

TUI Library

Ratatui (Rust)

For high-fidelity terminal dashboards.

Sandbox

Docker SDK / Firecracker

Isolation speed.

Orchestration

LangGraph (Python bridge) or Native Rust impl

Managing state loops.

Vector DB

Chroma (Local)

Fast retrieval for docs/memory.

7. Roadmap to MVP

[ ] Phase 1: The Shell. Build the Rust CLI that can hook into stdin/stdout and hold an API connection to Gemini 3.

[ ] Phase 2: The Cache. Implement the Context Caching mechanism to handle large repos efficiently.

[ ] Phase 3: The Sandbox. Create the Docker/Container interface for "Shadow Runs."

[ ] Phase 4: The Swarm. Connect Kimi k2.5 and implement the Architect/Worker pattern.

[ ] Phase 5: The UI. Build the localhost dashboard.