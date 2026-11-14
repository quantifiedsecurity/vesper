# Vesper website

The public language website at **https://vesper-lang.org/**. It presents the end-state language and ecosystem from the supplied Vesper Language Design & Formal Specification.

## Build

Requires Node.js 22 or newer. No package installation is needed.

```sh
cd website
npm run build
npm run check
```

Serve `website/dist` using any static HTTP server. The build includes the three compiler examples from the repository and checks that the displayed/downloaded source preserves their tokens. Code highlighting is generated as static HTML. JavaScript handles the file tabs, clipboard, navigation and the design skill's motion; it does not simulate compiler execution.

`site.css` and `site.js` contain Vesper-specific presentation and behavior. `assets/` contains the unchanged design-taste-frontend runtime files used by the page. `DESIGN.md` records the design choices and copy sources. Browser QA and the skill's finish gate run on `dist/index.html` with `--root dist`.

## Publish

`.github/workflows/website.yml` builds and checks the site on relevant pull requests. Pushes to `main` publish the built artifact to GitHub Pages, with the `github-pages` environment and OIDC. Changes to the included compiler examples also rebuild the site. The workflow can be run manually.

GitHub Pages uses the custom domain `vesper-lang.org`; `CNAME`, canonical URLs, the sitemap and share metadata all use that domain.

Cloudflare DNS (DNS only, proxy disabled):

| Type | Name | Target |
| --- | --- | --- |
| A | @ | 185.199.108.153 |
| A | @ | 185.199.109.153 |
| A | @ | 185.199.110.153 |
| A | @ | 185.199.111.153 |
| AAAA | @ | 2606:50c0:8000::153 |
| AAAA | @ | 2606:50c0:8001::153 |
| AAAA | @ | 2606:50c0:8002::153 |
| AAAA | @ | 2606:50c0:8003::153 |
| CNAME | www | robot-head.github.io |

GitHub provisions the TLS certificate after DNS validation. Enforce HTTPS in Pages settings once the certificate is available. See [GitHub's custom-domain documentation](https://docs.github.com/en/pages/configuring-a-custom-domain-for-your-github-pages-site/managing-a-custom-domain-for-your-github-pages-site).
