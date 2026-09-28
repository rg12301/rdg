# Third-party icon assets

Most `.svg` files in this directory are single-path fragments extracted from the
[Simple Icons](https://simpleicons.org) project
(https://github.com/simple-icons/simple-icons), licensed under
[CC0 1.0 Universal](https://creativecommons.org/publicdomain/zero/1.0/) (public domain).
Simple Icons' own `LICENSE.md` states brand icons "are the trademarks of their respective
companies... their use is subject to the trademark policies of the respective owners," which
applies here the same way it applies to Simple Icons itself: the *icon artwork* is CC0 and
freely redistributable, while the underlying brand names/marks remain the trademarks of their
owners. Each SVG below was fetched from
`https://raw.githubusercontent.com/simple-icons/simple-icons/develop/icons/<slug>.svg` and had
only its outer `<svg>`/`<title>` wrapper stripped and a `fill` attribute (the brand's published
hex color, from Simple Icons' own metadata) added to the `<path>` — the path data itself is
unmodified.

Vendored from Simple Icons — icon key (this project) → Simple Icons slug, where they differ:

- Languages: rust, python, go, typescript, javascript, kotlin, ruby, swift, php, scala,
  elixir, dart, r, perl, lua (same slug); `cpp` → `cplusplus`, `node` → `nodedotjs`
- Cloud/PaaS: googlecloud, cloudflare, vercel, netlify, digitalocean (same slug)
- Data stores: mysql, redis, mongodb, sqlite, elasticsearch, mariadb, neo4j, influxdb
  (same slug); `postgres` → `postgresql`, `cassandra` → `apachecassandra`,
  `kafka` → `apachekafka`, `rabbitmq` → `rabbitmq`, `cockroachdb` → `cockroachlabs`
- Infra/CI: docker, kubernetes, nginx, terraform, ansible, grafana, prometheus, jenkins,
  gitlab, github (same slug); `githubactions` → `githubactions`
- Frameworks: react, angular, django, flask, fastapi, express, dotnet, laravel (same slug);
  `vuejs` → `vuedotjs`, `spring` → `spring_creators`, `nextjs` → `nextdotjs`,
  `rails` → `rubyonrails`
- API/Auth: graphql, apollographql, stripe, auth0, okta (same slug)

## Not vendored from Simple Icons

Amazon and Microsoft requested removal of their entire brand-icon families from Simple Icons
some years ago, so **no AWS, DynamoDB, Azure, or C# icon exists in that dataset** — there is
nothing to vendor for these. `aws`, `dynamodb`, `csharp`, and `java` (Simple Icons has no
Oracle "Java coffee cup" mark either) remain the project's own hand-drawn flat approximations,
defined directly in `src/lib.rs`. There is currently no icon at all for Azure.

`database`, `table`, and `user` are generic (non-brand) glyphs, also hand-drawn in
`src/lib.rs`.
