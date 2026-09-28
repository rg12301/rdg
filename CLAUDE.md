# Project Agent Tooling Defaults

This project uses graphify by default for coding work in this repo.

## graphify
Turns the codebase into a queryable knowledge graph (local, deterministic AST parsing via tree-sitter -- no LLM, nothing leaves the machine for code).
- Installed via `uv tool install graphifyy` and registered for Claude Code with `graphify install --platform claude`.
- Graph already built for this repo: `graphify-out/graph.json`, `graphify-out/graph.html`, `graphify-out/GRAPH_REPORT.md`.
- Auto-rebuild is wired into git hooks (`post-commit`, `post-checkout`) and a git merge driver for `graphify-out/graph.json`.
- Use `/graphify .` in Claude Code (or `graphify query "..."`, `graphify path "A" "B"`, `graphify explain "X"`) before deep-diving into unfamiliar parts of this codebase.
- Repo: https://github.com/Graphify-Labs/graphify
