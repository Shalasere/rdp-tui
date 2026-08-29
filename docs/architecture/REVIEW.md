# Architecture proposal review

The files in this directory specify a proposed Rust/Ratatui rewrite of the
current Python `rdp-tui`. They are a design proposal and implementation
contract, not a description of the current codebase.

The original source archive is preserved as
`rdp-tui-architecture-contract-source.zip` with SHA-256:

```text
d716591ace6b17956a5a2c24f65e4b1e73cda42a7d7153d27954a366fff2660b
```

`04-amendments.yaml` is normative and resolves blockers found during review.
It takes precedence over conflicting original text. The significant amendments
are:

1. A detached session supervisor owns FreeRDP and any SSH tunnel until the RDP
   process exits. This prevents an unowned tunnel from surviving indefinitely
   after the launcher dies.
2. Credential references remain serializable data, while actual launch secrets
   live in a nonserializable, zeroizing `CredentialLease`. ASKPASS receives
   secrets through sealed inherited file descriptors rather than argv or plain
   environment values.
3. Previously referenced but undefined model types now have binding shapes.
4. Advanced FreeRDP arguments are a narrow, validated exception to the semantic
   profile boundary; credential and target switches are prohibited.
5. TOFU is the default certificate policy. Changed certificates are detected
   while FreeRDP is still running, surfaced with both fingerprints, and require
   explicit confirmation. Old pins are archived rather than deleted.
6. Import covers current `.rdp`, `.remmina`, Remmina-directory, JSON, and TOML
   workflows. Python migration has an explicit field map and secrets remain
   opt-in.
7. The machine-readable contract is versioned and must be parsed and
   cross-validated in CI before implementation changes are accepted.

The contract validator described in `04-amendments.yaml` is the first Rust
implementation slice. No functional porting should begin until that validator
can parse these files and enforce the dependency graph, stable identifiers,
and binding type references.
