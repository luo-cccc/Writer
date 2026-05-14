# Writer Web

Draft community site for [Writer](https://github.com/luo-cccc/Writer).

This site is not part of the current release path. It is kept in the repository
for later cleanup and should not be deployed until the copy, domain, Cloudflare
bindings, and public product positioning are reviewed.

Next.js 15 (App Router) + Tailwind, deployable to Cloudflare Workers via
[`@opennextjs/cloudflare`](https://opennext.js.org/cloudflare). Curated feed
content can call `deepseek-v4-flash` to summarize repository activity and store
the result in Workers KV.

## Local dev

```bash
cd web
npm install
cp .env.example .env.local   # fill in the keys you have
npm run dev                  # http://localhost:3000
```

Required env (only for the curator + private-repo rate limits):

| Variable            | What                                              | Required?            |
| ------------------- | ------------------------------------------------- | -------------------- |
| `DEEPSEEK_API_KEY`  | DeepSeek platform key (`sk-...`)                  | only for `/api/cron?task=curate` |
| `GITHUB_TOKEN`      | Fine-grained PAT, public-repo read scope          | optional (raises rate limit) |
| `GITHUB_REPO`       | Defaults to `luo-cccc/Writer`                    | optional             |
| `CRON_SECRET`       | Shared secret for manual cron invocation          | optional             |

The site renders fine without any of them — `Today's Dispatch` falls back to a static editorial; the GitHub feed shows "feed not yet loaded".

## Deploy to Cloudflare

Do not deploy this site as-is. When the web product is ready, choose the real
domain, replace placeholder KV ids, set secrets, and then use this deploy flow:

1. **Provision KV namespaces once:**

   ```bash
   npx wrangler kv namespace create CURATED_KV
   npx wrangler kv namespace create NEXT_INC_CACHE_KV
   ```

   Copy the printed `id` values into the matching `wrangler.jsonc` bindings
   (replace each `REPLACE_WITH_KV_ID`).

2. **Set secrets and deploy:**

   ```bash
   npx wrangler secret put DEEPSEEK_API_KEY
   npx wrangler secret put GITHUB_TOKEN     # optional
   npx wrangler secret put CRON_SECRET      # optional, for manual /api/cron?task=curate hits

   npm run deploy                           # builds with OpenNext + uploads
   ```

3. **Point the domain:** in the Cloudflare dashboard, add a Worker route for the
   chosen domain to the Writer Worker (the deploy command will offer this if the
   zone is already on your account).

The first cron run happens within 6 hours; you can also kick it manually:

```bash
curl -H "x-cron-secret: $CRON_SECRET" "https://<writer-domain>/api/cron?task=curate"
```

## What's where

```
web/
├── app/
│   ├── layout.tsx              root layout, font loading
│   ├── page.tsx                home — hero, dispatch, stats, how-it-works, join
│   ├── globals.css             design system: paper grain, hairlines, type, seal
│   ├── install/page.tsx        per-OS install with auto-detection
│   ├── docs/page.tsx           modes / tools / approval / config / mcp / providers
│   ├── feed/page.tsx           live mirror of issues + PRs
│   ├── roadmap/page.tsx        shipped / underway / considered / ruled out
│   ├── contribute/page.tsx     how to PR + house rules + dev loop
│   └── api/
│       ├── cron/route.ts          manual cron trigger: GitHub → DeepSeek → KV
│       └── github/feed/route.ts   cached JSON endpoint
├── components/
│   ├── nav.tsx                 sticky header w/ date strip + CJK accents
│   ├── footer.tsx              dense 5-column footer
│   ├── seal.tsx                red Chinese-seal mark used as section anchor
│   ├── ticker.tsx              animated live activity strip
│   ├── stat-grid.tsx           tabular repo stats row
│   ├── feed-card.tsx           one issue/PR card
│   └── install-tabs.tsx        client component, OS auto-detect + copy
├── lib/
│   ├── types.ts                shared types
│   ├── github.ts               REST client + relative-time formatter
│   ├── deepseek.ts             v4-flash chat client + curate() prompt
│   └── kv.ts                   Cloudflare KV access via OpenNext bindings
├── wrangler.jsonc              CF Worker config + cron + KV binding
├── open-next.config.ts         OpenNext adapter config
└── tailwind.config.ts          design tokens
```

## Aesthetic

"Yamen tech": Qing memorial document × WeChat news feed × Bloomberg terminal.

- **Palette**: cream paper `#FAF6EE`, ink `#0A2540`, cinnabar red `#C8102E`, aged gold, jade green, cobalt blue.
- **Type**: Fraunces (display), IBM Plex Sans (body), JetBrains Mono (UI/code), Noto Serif SC (decorative CJK anchors).
- **Structure**: hairline 1px dividers, multi-column grids, big tabular numbers, surgical use of red for "hot" markers, decorative Chinese-seal squares as section anchors.

If you want to retune the palette, edit `:root` in `app/globals.css` and the `colors` block in `tailwind.config.ts`.
