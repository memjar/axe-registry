# AXE Platform Review — Findings (2026-07-17, Nova/JL1)

Left here per James's request (keep all repos; document, don't archive). Full north-star:
`~/Downloads/AXE-PLATFORM-NORTH-STAR.md` + Brain id `3cb93bca6bae`. Source: 5-agent deep review.

## Repo landscape (234 memjar repos, ~65 pushed this week)
The week shipped an owned layer per third-party dependency: camel↔Ollama, Crown↔Obsidian,
Chorus↔Slack, **axe-pkg↔npm**, Reactor↔manual-finetuning, Albus/DSC↔borrowed-weights,
KeyHub/authgate↔auth, axeonl↔control-plane, Butler↔edge-proxy.

## Memory/knowledge sprawl — KEEP ALL (per James), consolidation is advisory only
~45 memory/knowledge repos overlap. If/when consolidating later, the canonical set appears to be:
- **KEEP:** axe-crown (vault), axe-memory (unified memory), compose (editor), imi-obsidian (IMI tenant),
  brain-rs/axe-brain (Brain API). **axe-obsidian = LIVE Crown vault backend (CROWN_GITHUB_REPO) — never archive.**
- **Advisory-archive-later (NOT done):** axe-halo (self-declares superseded by Crown), axe-onix, axe-notion,
  axe-vault, axe-mem, axe-mempalace, axe-memunlocked, axe-palace, axe-recall, axe-memory-gateway,
  axe-memory-kit, axe-axenetwork, memoryjar, mum-memory, axe-oracle, llama-brain, Aeterna, Mjbeta.
  All pre-June, feature-overlapping, none referenced as canonical. Left in place per James.

## Skills / MCP sprawl (advisory)
3 skill systems: ~/.claude/skills (48 curated SKILL.md — canonical agent-facing), ~/.axe/skills (126 legacy
skill_NN.py — supersede-able), ~/.axe/skills-hub (MCP serving layer — keep). MCP itself split: AXeGoMCP (135)
+ axe-mcp (77) + skills-hub — unify eventually.

## Security notes (for the KeyHub cutover — keys go through authgate-keyhub as single source of truth)
- `XJVO…` Brain key is shipped in browser HTML (_neural/_dispatch/_tasks.html) + committed to git, and reused
  for Brain + Butler + PocketBase (one leak = full blast radius). Rotate via KeyHub (planned cutover).
- **This repo (axe-pkg) is PUBLIC but named "Private Package Registry"** — flagged for James's decision (left as-is).
- Chorus defaults axe-admin-2026 / axess and the fleet key are committed; move to KeyHub-issued scoped keys.
- brain-rs v3.0.0 API changed: /api/save now needs `title` + `tags` as a STRING (not array). The axe-brain
  skill documents the old shape — update it.

## Platform direction
P0 stabilize spine (Chorus hardened ✓; KeyHub = one source of truth for keys); P1 close the flywheel
(highest ROI — see Reactor); P2 golden paths (deploy/publish/tool); P3 productize multi-tenancy (IMI → revenue).
Thesis: entropy, not capability. Integrate + harden what's built; fight sprawl.
