# New-repo templates for `bynk-lang/vscode-bynk`

Drafts of the files the extracted repository needs (extraction plan §5, §6),
kept here, outside any active `.github/`, until the repository exists. They are
**untested on a runner and not linted with actionlint**: the first run in the
new repo is the real test, so expect to fix details then. Paths assume the
extension is at the repo root (the `filter-repo` split, §3).

| File | Does |
|---|---|
| `.github/workflows/ci.yml` | Build, contract check, web bundle + smoke tests, standalone package, integration tests against the pinned release, pin check; `CI green` gate |
| `.github/workflows/compat.yml` | Nightly: build the compiler from `bynk-lang/bynk@main`, run the integration suite and contract check against it, open an issue on failure |
| `.github/workflows/pin-bump.yml` | On a compiler release (`repository_dispatch: bynk-release`), open a PR moving `bynkServerVersion`, auto-merge on green |
| `.github/workflows/release.yml` | On an extension tag: verify, package, publish to the Marketplace (Entra ID, no stored secret) |
| `scripts/next-server-pin.sh`, `scripts/check-server-pin.sh` | The pin scripts, adapted to the repo root and without `release.yml`'s target matrix |

## Needs from outside the repository

- The repository itself, its `main` ruleset (required check: `CI green`; merge queue if used), and the
  setting **Allow auto-merge** (`pin-bump.yml` calls `gh pr merge --auto`, which fails without it).
- A GitHub App installed on it for `pin-bump.yml` (a PR opened with `GITHUB_TOKEN` runs no
  workflows) with `contents: write` and `pull-requests: write`. Secrets
  `BOT_APP_ID` / `BOT_APP_KEY`.
- Nothing for the `compat` label: `compat.yml` creates it, and keeps a single
  open issue rather than one per failing night.
- The Marketplace credential: an Entra federated credential trusting this repo's
  `vscode-marketplace` environment (variables `AZURE_CLIENT_ID`,
  `AZURE_TENANT_ID`) — the old repo's binding does not carry over.
- On the compiler side, a `notify-extension` job in `release.yml` (below) and an
  App token with access to both repositories.

### Compiler-side job (add to `bynk-lang/bynk`'s `release.yml` at cutover)

```yaml
  notify-extension:
    name: Tell the extension repository about this release
    needs: github-release
    if: github.event_name == 'push' # a tag push; the GitHub Release exists
    runs-on: ubuntu-latest
    timeout-minutes: 5
    steps:
      - id: app-token
        uses: actions/create-github-app-token@64ab729a6294000f5aa4a4bc0b73d1f32bc45510 # v3.3.0
        with:
          app-id: ${{ secrets.STAMP_APP_ID }}
          private-key: ${{ secrets.STAMP_APP_KEY }}
          repositories: vscode-bynk
      - env:
          GH_TOKEN: ${{ steps.app-token.outputs.token }}
        run: gh api repos/bynk-lang/vscode-bynk/dispatches -f event_type=bynk-release -f "client_payload[tag]=$GITHUB_REF_NAME"
```

The old `server-pin`, `vsix` and `publish-vscode` jobs are deleted in the same
change.
