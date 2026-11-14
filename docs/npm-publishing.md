# npm publication (maintainers)

The `npm-publish` job in `.github/workflows/release.yml` publishes `@engramo/mcp` and its five platform packages
(`@engramo/mcp-{darwin-arm64,darwin-x64,linux-arm64,linux-x64,win32-x64}`) with
[npm trusted publishing](https://docs.npmjs.com/trusted-publishers). There is no npm token, so nothing expires or
needs rotating. npm exchanges the job's GitHub OIDC token (`id-token: write`) for a short-lived publish token and
signs the provenance attestation automatically. Trusted publishing requires npm >= 11.5.1, which the job checks.

## One-time setup

1. For **each** of the six packages on npmjs.com: Settings > Trusted Publisher > GitHub Actions, with
   - Organization or user: the owner of this repo
   - Repository: `engram-mcp`
   - Workflow filename: `release.yml`
   - Environment name: `npm`

   The environment is required, not optional. `mcpb-attest` also has `id-token: write`, and without the
   environment binding npm would accept that job's OIDC token as well.
2. The `npm` GitHub environment is created automatically on the first run that references it. Optionally restrict
   it (Settings > Environments > `npm`) to `main` and `v*` tags. Do not add required reviewers unless you want
   dry runs gated too, since the job runs on dry runs.
3. Once a real release has published through OIDC, harden each package: Settings > Publishing access >
   "Require two-factor authentication and disallow tokens". Then delete the `NPM_TOKEN` repo secret and revoke the
   old npm token.

## Troubleshooting

- `404 Not Found - PUT https://registry.npmjs.org/@engramo%2f...`: the package has no matching trusted publisher.
  Check the repository, workflow filename, and environment name on that package.
- Trusted publishing works only on GitHub-hosted runners and only for packages that already exist on the registry.
