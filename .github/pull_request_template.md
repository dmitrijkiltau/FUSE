## Summary

Describe what changed and why.

## Change type

- [ ] Language/static semantics
- [ ] Runtime semantics/boundary behavior
- [ ] Tooling/CLI/LSP
- [ ] Docs/site only
- [ ] Test only

## Required checks run

- [ ] `scripts/semantic_suite.sh` (when semantics changed)
- [ ] `scripts/authority_parity.sh` (when semantics/runtime/backends changed)
- [ ] `scripts/release_smoke.sh` (for release-critical changes)
- [ ] `scripts/fuse check --manifest-path docs` (when docs site changed)

Paste key command results:

```text
<results>
```

## Specs/docs updated

- [ ] `fls.md` (if syntax/static semantics changed)
- [ ] `runtime.md` (if runtime behavior changed)
- [ ] `README.md` (if user workflow changed)
- [ ] Other docs/spec files updated as needed

## Compatibility

- [ ] Backward compatible
- [ ] Deprecation introduced
- [ ] Breaking change

If deprecation or breaking change, include migration notes.

## RFC

- [ ] RFC required
- [ ] RFC not required

RFC link (if required): `<link>`
