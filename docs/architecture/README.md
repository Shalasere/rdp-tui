Machine-oriented companion to `rdp-tui-architecture-contract-final.md`. Not written for prose readability — written to be loaded into a coding agent's context alongside a task.

Read order for an implementing agent:

1. `REVIEW.md` — proposal status, provenance, and review outcome.
2. `04-amendments.yaml` — normative corrections; this overrides conflicting original clauses.
3. `00-manifest.yaml` — module list, dependency graph, forbidden edges, file layout.
4. `01-types.yaml` — target type shapes per module.
5. `02-rules.yaml` — invariants, decisions, and grep-able anti-patterns.
6. `03-process.yaml` — XDG paths, commands, diagnostics, tests, and phase plan.
7. `rdp-tui-architecture-contract-final.md` — original human-readable contract.

If a generated change violates `04-amendments.yaml` or an unamended entry in
`02-rules.yaml`, that is a contract violation regardless of whether tests pass.
Stable IDs (`INV-*`, `DEC-*`, `AP-*`, `GAP-*`) should be cited when code exists
specifically to satisfy one.
