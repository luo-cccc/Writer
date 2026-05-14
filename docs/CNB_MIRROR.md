# CNB Cool Mirror

CNB mirroring is optional for Writer. The repository does not assume a live CNB
remote, and release success must not depend on CNB unless a maintainer has
explicitly configured one.

The workflow at [../.github/workflows/sync-cnb.yml](../.github/workflows/sync-cnb.yml)
is dispatch-only. To use it, configure the `CNB_GIT_TOKEN` repository secret and
run the workflow manually with the real CNB repository path.

## Manual Workflow

```bash
gh workflow run sync-cnb.yml \
  --repo luo-cccc/Writer \
  -f cnb_repo=<cnb-owner>/<repo>
```

The workflow pushes the triggering ref to:

```text
https://cnb.cool/<cnb_repo>.git
```

## Manual Git Fallback

```bash
git remote add cnb https://cnb:${CNB_TOKEN}@cnb.cool/<cnb-owner>/<repo>.git
git fetch origin
git checkout main
git reset --hard origin/main
git push cnb main --force
git push cnb vX.Y.Z
```

Use the fallback only after confirming the CNB destination exists and the token
has push scope.

## Release Boundary

CNB is a source mirror only. It does not host GitHub Release binaries,
checksums, npm packages, crates, or Docker images. The supported release sources
remain:

- GitHub Releases under `https://github.com/luo-cccc/Writer/releases`
- npm package metadata for the existing `deepseek-tui` wrapper
- crates.io packages for the existing Rust crate names
- GHCR image `ghcr.io/luo-cccc/writer`
