# FUSE Guides

GitHub-readable guide surface for FUSE.

## Guides

- [Onboarding](onboarding.md)
- [Boundary Contracts](boundary-contracts.md)
- [Developer Reference](reference.md)
- [Migration: 0.8.x -> 0.9.0](migrations/0.8-to-0.9.md)

## Regeneration

`onboarding.md` and `boundary-contracts.md` are generated from `guides/src/*.fuse`:

```bash
./scripts/generate_guide_docs.sh
```

`reference.md` is hand-maintained alongside `spec/fls.md` and `spec/runtime.md`.
Update it in the same PR whenever observable language behavior changes.
