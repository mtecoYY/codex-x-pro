# Codex Star History Worker

Cloudflare Worker that serves near-real-time Star History SVGs for:

- `mtecoYY/codex-x-pro`
- `yynxxxxx/Codex-5.5-codex-instruct-5.5`

The Worker stores only aggregated timestamps and counts in KV. The historical baseline is bootstrapped once with an authenticated repository-owner session because GitHub no longer exposes timestamped Stargazers to repository `GITHUB_TOKEN` identities. After that, repository-scoped GitHub Actions reconcile the public total every 15 minutes, while GitHub `star` webhooks provide faster updates. Successful refreshes also purge the known GitHub Camo chart URLs so README images do not remain stuck on an older SVG. No personal GitHub token is stored in Cloudflare or repository secrets.

## Endpoints

```text
GET  /v1/charts/codex-x-pro.svg?theme=light
GET  /v1/charts/codex-x-pro.svg?theme=dark
GET  /v1/charts/codex-5-5.svg?theme=light
GET  /v1/charts/codex-5-5.svg?theme=dark
GET  /v1/data/codex-x-pro
GET  /v1/data/codex-5-5
GET  /healthz
POST /v1/refresh
POST /v1/github/webhook
```

## Secrets

Set secrets through Wrangler; never commit them:

```bash
npx wrangler secret put INGEST_TOKEN
npx wrangler secret put WEBHOOK_SECRET
```

`INGEST_TOKEN` must also be stored as the `STAR_HISTORY_INGEST_TOKEN` Actions secret in both repositories. `WEBHOOK_SECRET` is configured on both GitHub repository webhooks.

## Verify And Deploy

```bash
npm test
npx wrangler deploy --dry-run
npx wrangler deploy
```
