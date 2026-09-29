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

## Full-colour logos (`color/`)

The `.svg` files in [`color/`](color/) are the full-colour ("original", or "plain" where no
original exists) variants from the [devicon](https://devicon.dev) project
(https://github.com/devicons/devicon), used unmodified. devicon is distributed under the
MIT License:

> The MIT License (MIT)
>
> Copyright (c) 2015 konpa
>
> Permission is hereby granted, free of charge, to any person obtaining a copy of this
> software and associated documentation files (the "Software"), to deal in the Software
> without restriction, including without limitation the rights to use, copy, modify, merge,
> publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons
> to whom the Software is furnished to do so, subject to the following conditions:
>
> The above copyright notice and this permission notice shall be included in all copies or
> substantial portions of the Software.
>
> THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED,
> INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR
> PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE
> FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR
> OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
> DEALINGS IN THE SOFTWARE.

As with Simple Icons, the logos themselves remain trademarks of their respective owners.
Each file was copied from `icons/<dir>/<file>` in the devicon repository; icon key (this
project) → devicon file:

- `airflow` → `apacheairflow/apacheairflow-original.svg`
- `android` → `android/android-original.svg`
- `angular` → `angular/angular-original.svg`
- `ansible` → `ansible/ansible-original.svg`
- `apache` → `apache/apache-original.svg`
- `apollographql` → `apollographql/apollographql-original.svg`
- `apple` → `apple/apple-original.svg`
- `argocd` → `argocd/argocd-original.svg`
- `aws` → `amazonwebservices/amazonwebservices-original-wordmark.svg`
- `azure` → `azure/azure-original.svg`
- `bash` → `bash/bash-original.svg`
- `bun` → `bun/bun-original.svg`
- `c` → `c/c-original.svg`
- `cassandra` → `cassandra/cassandra-original.svg`
- `clickhouse` → `clickhouse/clickhouse-original.svg`
- `clojure` → `clojure/clojure-original.svg`
- `cloudflare` → `cloudflare/cloudflare-original.svg`
- `consul` → `consul/consul-original.svg`
- `cosmosdb` → `cosmosdb/cosmosdb-original.svg`
- `couchbase` → `couchbase/couchbase-original.svg`
- `couchdb` → `couchdb/couchdb-original.svg`
- `cpp` → `cplusplus/cplusplus-original.svg`
- `csharp` → `csharp/csharp-original.svg`
- `dart` → `dart/dart-original.svg`
- `datadog` → `datadog/datadog-original.svg`
- `deno` → `denojs/denojs-original.svg`
- `digitalocean` → `digitalocean/digitalocean-original.svg`
- `django` → `django/django-plain.svg`
- `docker` → `docker/docker-original.svg`
- `dotnet` → `dot-net/dot-net-original.svg`
- `duckdb` → `duckdb/duckdb-original.svg`
- `dynamodb` → `dynamodb/dynamodb-original.svg`
- `dynatrace` → `dynatrace/dynatrace-original.svg`
- `elasticsearch` → `elasticsearch/elasticsearch-original.svg`
- `elixir` → `elixir/elixir-original.svg`
- `envoy` → `envoy/envoy-original.svg`
- `erlang` → `erlang/erlang-original.svg`
- `express` → `express/express-original.svg`
- `fastapi` → `fastapi/fastapi-original.svg`
- `fastify` → `fastify/fastify-original.svg`
- `firebase` → `firebase/firebase-original.svg`
- `flask` → `flask/flask-original.svg`
- `flutter` → `flutter/flutter-original.svg`
- `fsharp` → `fsharp/fsharp-original.svg`
- `github` → `github/github-original.svg`
- `githubactions` → `githubactions/githubactions-original.svg`
- `gitlab` → `gitlab/gitlab-original.svg`
- `go` → `go/go-original.svg`
- `googlecloud` → `googlecloud/googlecloud-original.svg`
- `grafana` → `grafana/grafana-original.svg`
- `graphql` → `graphql/graphql-plain.svg`
- `grpc` → `grpc/grpc-original.svg`
- `hadoop` → `hadoop/hadoop-original.svg`
- `haskell` → `haskell/haskell-original.svg`
- `helm` → `helm/helm-original.svg`
- `heroku` → `heroku/heroku-original.svg`
- `influxdb` → `influxdb/influxdb-original.svg`
- `jaeger` → `jaegertracing/jaegertracing-original.svg`
- `java` → `java/java-original.svg`
- `javascript` → `javascript/javascript-original.svg`
- `jenkins` → `jenkins/jenkins-original.svg`
- `julia` → `julia/julia-original.svg`
- `kafka` → `apachekafka/apachekafka-original.svg`
- `kibana` → `kibana/kibana-original.svg`
- `kotlin` → `kotlin/kotlin-original.svg`
- `kubernetes` → `kubernetes/kubernetes-original.svg`
- `laravel` → `laravel/laravel-original.svg`
- `linux` → `linux/linux-plain.svg`
- `logstash` → `logstash/logstash-original.svg`
- `lua` → `lua/lua-original.svg`
- `mariadb` → `mariadb/mariadb-original.svg`
- `memcached` → `memcached/memcached-original.svg`
- `mongodb` → `mongodb/mongodb-original.svg`
- `mssql` → `microsoftsqlserver/microsoftsqlserver-original.svg`
- `mysql` → `mysql/mysql-original.svg`
- `nats` → `nats/nats-original.svg`
- `neo4j` → `neo4j/neo4j-original.svg`
- `nestjs` → `nestjs/nestjs-original.svg`
- `netlify` → `netlify/netlify-original.svg`
- `newrelic` → `newrelic/newrelic-original.svg`
- `nextjs` → `nextjs/nextjs-original.svg`
- `nginx` → `nginx/nginx-original.svg`
- `node` → `nodejs/nodejs-original.svg`
- `nomad` → `nomad/nomad-original.svg`
- `oauth` → `oauth/oauth-original.svg`
- `okta` → `okta/okta-original.svg`
- `openapi` → `openapi/openapi-original.svg`
- `openstack` → `openstack/openstack-original.svg`
- `opentelemetry` → `opentelemetry/opentelemetry-original.svg`
- `oracle` → `oracle/oracle-original.svg`
- `perl` → `perl/perl-original.svg`
- `php` → `php/php-original.svg`
- `podman` → `podman/podman-original.svg`
- `postgres` → `postgresql/postgresql-original.svg`
- `postman` → `postman/postman-original.svg`
- `prisma` → `prisma/prisma-original.svg`
- `prometheus` → `prometheus/prometheus-original.svg`
- `pulsar` → `pulsar/pulsar-original.svg`
- `pulumi` → `pulumi/pulumi-original.svg`
- `python` → `python/python-original.svg`
- `quarkus` → `quarkus/quarkus-original.svg`
- `r` → `r/r-original.svg`
- `rabbitmq` → `rabbitmq/rabbitmq-original.svg`
- `rails` → `rails/rails-plain.svg`
- `react` → `react/react-original.svg`
- `redis` → `redis/redis-original.svg`
- `rocksdb` → `rocksdb/rocksdb-original.svg`
- `ruby` → `ruby/ruby-original.svg`
- `rust` → `rust/rust-original.svg`
- `salesforce` → `salesforce/salesforce-original.svg`
- `scala` → `scala/scala-original.svg`
- `sentry` → `sentry/sentry-original.svg`
- `slack` → `slack/slack-original.svg`
- `socketio` → `socketio/socketio-original.svg`
- `spark` → `apachespark/apachespark-original.svg`
- `splunk` → `splunk/splunk-original-wordmark.svg`
- `spring` → `spring/spring-original.svg`
- `sqlite` → `sqlite/sqlite-original.svg`
- `supabase` → `supabase/supabase-original.svg`
- `surrealdb` → `surrealdb/surrealdb-original.svg`
- `svelte` → `svelte/svelte-original.svg`
- `swagger` → `swagger/swagger-original.svg`
- `swift` → `swift/swift-original.svg`
- `terraform` → `terraform/terraform-original.svg`
- `tomcat` → `tomcat/tomcat-original.svg`
- `traefik` → `traefikproxy/traefikproxy-original.svg`
- `twilio` → `twilio/twilio-original.svg`
- `typescript` → `typescript/typescript-original.svg`
- `ubuntu` → `ubuntu/ubuntu-original.svg`
- `vault` → `vault/vault-original.svg`
- `vercel` → `vercel/vercel-original.svg`
- `vuejs` → `vuejs/vuejs-original.svg`
- `yugabytedb` → `yugabytedb/yugabytedb-original.svg`
- `zig` → `zig/zig-original.svg`
