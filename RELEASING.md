# Release guide / 发布指南

Aeon Memory uses one semantic version for the Rust workspace and the
`@aeon-memory/opencode` npm package. A signed or annotated `vX.Y.Z` tag is the
only production release trigger.

## One-time repository setup

1. Keep the GitHub repository public so npm can generate provenance.
2. On npmjs.com, open `@aeon-memory/opencode` package settings and configure a
   **Trusted Publisher** with:
   - provider: GitHub Actions
   - organization or user: `HuChundong`
   - repository: `aeon-memory`
   - workflow filename: `release.yml`
   - allowed action: `npm publish`
3. Protect `main` and release tags in GitHub. Require the `CI` checks before merge.
4. Enable private vulnerability reporting and Discussions where available.

Trusted Publishing uses GitHub OIDC and does not require an `NPM_TOKEN` secret.
The release workflow grants `id-token: write` only to the npm publish job and
runs on a GitHub-hosted runner.

## Release procedure

1. Update the same `X.Y.Z` in:
   - `[workspace.package].version` in `Cargo.toml`;
   - `integrations/opencode/package.json` and its lockfile;
   - root `package.json` and its lockfile.
2. Move user-visible entries from `Unreleased` into a dated `X.Y.Z` section in
   `CHANGELOG.md`.
3. Run the complete checks from `CONTRIBUTING.md` and inspect `npm pack --dry-run`.
4. Merge the release commit to `main` and confirm CI is green.
5. Create and push an annotated tag:

```bash
git tag -a vX.Y.Z -m "Aeon Memory vX.Y.Z"
git push origin vX.Y.Z
```

The `Release` workflow then:

- validates the tag and Cargo/npm version alignment;
- repeats formatting, lint, Rust tests, plugin tests, and package inspection;
- builds five native archives with verified sqlite-vec binaries;
- creates `SHA256SUMS` and a GitHub Release;
- publishes `@aeon-memory/opencode@X.Y.Z` through npm Trusted Publishing;
- verifies the public npm registry after publication.

If any job fails, fix the underlying problem and publish a new version. Never
move or overwrite a tag that users may already have fetched, and never attempt
to overwrite an npm version.

## 中文摘要

正式发布只由 `vX.Y.Z` 标签触发。发布前必须保证 Cargo workspace、根数据工具和 OpenCode
npm 包版本完全一致，主分支 CI 通过，CHANGELOG 已更新。npm 通过 GitHub OIDC Trusted
Publisher 发布，不保存长期 Token。标签一旦推送或 npm 版本一旦发布，不得覆盖；失败时
修复并递增版本重新发布。

