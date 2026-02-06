# Project Nexus Roadmap

> Next-Generation AI CLI: State-First Architecture with Verified Execution

**Status:** In Planning | **Target Start:** Feb 2026

---

## Phase 1: The Shell (Foundation)
**Goal:** Functional Rust CLI with API connectivity and basic command interception
**ETA:** 4-6 weeks

### Milestones
- [ ] Initialize Rust project structure with Cargo
- [ ] Implement stdin/stdout hooking mechanism
- [ ] Build multi-provider API system:
  - [ ] Provider trait abstraction
  - [ ] OpenCode API integration
  - [ ] OpenRouter API integration (multi-model proxy)
  - [ ] Google Gemini API (OAuth + API key)
  - [ ] Claude Code API (OAuth flow)
- [ ] Configuration system (provider switching, auth management)
- [ ] Create basic TUI using Ratatui (file tree, diff viewer skeleton)
- [ ] Command interceptor (detect and queue shell commands)

### Deliverables
- Working binary: `nexus` command in terminal
- Can connect to Gemini 3 Pro API
- Basic terminal UI showing current state
- Commands are intercepted but NOT executed (dry-run mode)

---

## Phase 2: The Cache (Context Engine)
**Goal:** Efficient context management for large repositories
**ETA:** 3-4 weeks | **Dependencies:** Phase 1

### Milestones
- [ ] Implement file tree caching mechanism
- [ ] Build diff detection system (track file changes)
- [ ] Create "warm handshake" protocol for initial context load
- [ ] Optimize token usage (only send diffs after handshake)
- [ ] Add Chroma vector store integration
- [ ] Implement Mem0-style long-term memory for user preferences

### Deliverables
- Can load 100k+ line repos without token overflow
- Subsequent requests only transmit changed files
- Remembers user preferences across sessions

---

## Phase 3: The Sandbox (Safety Layer)
**Goal:** Isolated execution environment for all shell commands
**ETA:** 4-5 weeks | **Dependencies:** Phase 2

### Milestones
- [ ] Docker SDK integration (container spawning)
- [ ] Build filesystem mirroring (host → container)
- [ ] Implement "Shadow Run" execution loop
  - [ ] Intercept command
  - [ ] Run in container
  - [ ] Capture exit code and output
- [ ] Create test runner integration
- [ ] Build hydration system (container → host on success)
- [ ] Rollback mechanism on failure

### Deliverables
- All shell commands run in Docker first
- Pass/fail gate before host filesystem changes
- Users can inspect container state before hydration

---

## Phase 4: The Swarm (Orchestration)
**Goal:** Parallel agent execution via Kimi k2.5
**ETA:** 6-8 weeks | **Dependencies:** Phase 3

### Milestones
- [ ] LangGraph integration (state machine)
- [ ] Implement Architect Agent
  - [ ] Task decomposition logic
  - [ ] Dependency graph builder
- [ ] Build Worker Agent factory
  - [ ] Frontend Worker
  - [ ] Backend Worker
  - [ ] QA Worker
- [ ] Create parallel execution scheduler
- [ ] Implement Git Merger (conflict resolution)
- [ ] Build self-correction loop (on Shadow Run failure)

### Deliverables
- Can say "Refactor auth system" → parallel execution across multiple files
- Workers collaborate without blocking
- Auto-merge with conflict resolution

---

## Phase 5: The Interface (Visualization)
**Goal:** Web dashboard and enhanced CLI experience
**ETA:** 4-5 weeks | **Dependencies:** Phase 4

### Milestones
- [ ] Build headless daemon mode
- [ ] Create localhost web server (port 8888)
- [ ] Design PWA dashboard
  - [ ] Swarm visualization (real-time agent activity)
  - [ ] Visual diff reviewer
  - [ ] "Big Red Button" kill switch
  - [ ] Project settings and MCP config UI
- [ ] Rich CLI diff viewer
- [ ] Notification system (desktop alerts for proposed fixes)

### Deliverables
- localhost:8888 shows live agent activity
- Visual diff review before applying changes
- Mobile-friendly PWA for monitoring

---

## Phase 6: Integration & Intelligence
**Goal:** MCP compliance and self-healing capabilities
**ETA:** 4-5 weeks | **Dependencies:** Phase 5

### Milestones
- [x] MCP server implementation
- [x] Database adapters (SQLite, Postgres via MCP)
- [x] External tool connectors (GitHub, Sentry, Slack)
- [ ] Self-Healing Watcher
  - [x] File system watcher integration
  - [ ] Dev server log monitoring
  - [ ] Stack trace detection (regex + AI)
  - [ ] Auto-investigate and propose fixes
- [ ] MCP marketplace/config UI

### Deliverables
- Connects to any MCP-compliant tool
- Proactively detects build errors and suggests fixes
- One-click integration setup

---

## Phase 7: Hardening & Release
**Goal:** Production-ready with comprehensive testing
**ETA:** 3-4 weeks | **Dependencies:** Phase 6

### Milestones
- [ ] End-to-end test suite (all phases)
- [ ] Performance benchmarks (large repos, concurrent agents)
- [ ] Security audit (sandbox escape prevention)
- [ ] Documentation (API, user guide, architecture)
- [ ] Beta program (invite-only)
- [ ] Build release pipeline (cross-platform binaries)

### Deliverables
- MVP release for public use
- Comprehensive docs
- Stable binaries for macOS, Linux, Windows

---

## Technical Stack Summary

| Component | Technology |
|-----------|------------|
| Core Binary | Rust |
| TUI | Ratatui |
| Sandbox | Docker SDK (Phase 3), Firecracker (Phase 5+) |
| Orchestration | LangGraph |
| Vector DB | Chroma (local) |
| Context Engine | Gemini 3 Pro |
| Swarm Logic | Kimi k2.5 |
| Web Dashboard | PWA (React/Vue + WebSocket) |
| Protocol | MCP 2026 |

---

## Risk Mitigation

1. **Sandbox Performance** - Start with Docker, migrate to Firecracker if latency is unacceptable
2. **Model Rate Limits** - Implement aggressive caching, queueing, and fallback logic
3. **Git Conflicts** - Extensive testing on the Merger component; manual override always available
4. **Security** - No secrets in containers; host filesystem read-only until hydration

---

## Success Metrics

- [ ] Can handle repos up to 500k lines without context loss
- [ ] Shadow Run verification < 5 seconds for typical commands
- [ ] Parallel agent execution (4+ workers simultaneously)
- [ ] Zero host filesystem corruption incidents
- [ ] Self-healing detects 90%+ of common build errors

---

*Last Updated: Feb 2026*
