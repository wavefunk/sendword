# Website Deployment

The website is deployed to Cloudflare Pages by Woodpecker from
`.woodpecker/website.yml`.

The workflow runs on pushes to `main` that touch `website/**` or the workflow
itself, and it can also be started manually in Woodpecker. It builds
`website/` with `ghcr.io/wavefunk/eigen:0.17.1`, then uploads `website/dist`
to the Cloudflare Pages project named `sendword` with Wrangler.

## Cloudflare Setup

Create a Direct Upload Pages project with production branch `main`:

```sh
npx wrangler pages project create sendword --production-branch main
```

In Cloudflare, add `sendword.online` as a zone and update the registrar to use
Cloudflare's assigned nameservers. After Cloudflare marks the zone active, open
Workers & Pages, select the `sendword` Pages project, go to Custom domains, and
add `sendword.online`.

Cloudflare should create the DNS record for the apex domain when the custom
domain is attached. Add `www.sendword.online` as another custom domain only if
the `www` hostname should also serve the site.

If the Pages project name cannot be `sendword`, change `CLOUDFLARE_PAGES_PROJECT`
in `.woodpecker/website.yml` to the project name that Cloudflare created.

## Woodpecker Secrets

Add these repository secrets in Woodpecker:

- `cloudflare_api_token`: Cloudflare API token with `Account > Cloudflare Pages > Edit`.
- `cloudflare_account_id`: Cloudflare account ID for the account that owns `sendword.online`.

Limit both secrets to push and manual events. If Woodpecker allows image/plugin
filtering for these secrets, restrict them to `node:22-slim`.

## Cloudflare API Token

Create a custom API token in Cloudflare:

1. Go to My Profile > API Tokens > Create Token.
2. Use Custom Token.
3. Add permission `Account > Cloudflare Pages > Edit`.
4. Scope it to the account that owns the `sendword` Pages project.
5. Store the token value as the Woodpecker `cloudflare_api_token` secret.

Find the account ID from the Cloudflare dashboard account or zone overview and
store it as the Woodpecker `cloudflare_account_id` secret.

## Verification

After the first successful Woodpecker deploy, check the deployment URL shown by
Wrangler, then check `https://sendword.online`. If the custom domain is still
pending, wait for nameserver activation and Cloudflare certificate issuance to
finish before debugging the pipeline.
