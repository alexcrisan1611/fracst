# fracst-gateway

A Cloudflare Worker that lets people render on your fal.ai GPU with their
own fracst keys (`fst_…`), which you can limit and revoke. Your fal key stays
here, as a secret. It relays the fracst protocol unchanged.

```
npm install
npx wrangler login
# in wrangler.jsonc: "FAL_APP": "<your-fal-user>/fracst"
npx wrangler secret put FAL_KEY
npx wrangler secret put ADMIN_TOKEN
npx wrangler deploy

FRACST_GATEWAY=https://fracst-gateway.<sub>.workers.dev FRACST_ADMIN_TOKEN=... \
  npm run keys -- create alice --minutes 600
```

See [docs/remote.md](../docs/remote.md#sharing-it-the-cloudflare-gateway)
for the whole setup, limits and local testing (`npm run dev`, `npm run smoke`).

- `src/index.ts`: routes, key checks, admin API.
- `src/session.ts`: the per-connection Durable Object that relays to the GPU.
- `src/registry.ts`: the Durable Object with keys, usage and open sessions.
- `scripts/keys.mjs`: key management from the command line.
