//! Built-in flat vector icons for programming languages, cloud providers, database
//! engines, infra tooling, web frameworks, and actors.
//!
//! Most icons are vendored from the [Simple Icons](https://simpleicons.org) dataset
//! (CC0 / public domain — see `assets/THIRD_PARTY_LICENSES.md`) as single-path SVG
//! fragments under `assets/`, embedded at compile time via `include_str!` so the tool
//! stays fully offline/deterministic at both build and run time. A handful of icons for
//! brands that Simple Icons does not carry (Java, C#, AWS, DynamoDB — see the license
//! file for why) remain hand-drawn flat approximations, as do the fully generic
//! (non-brand) database/table/user glyphs.
//!
//! Icons can be rendered directly into SVG diagrams or base64-encoded for
//! draw.io embedded HTML images.

use base64::Engine as _;
use std::collections::HashMap;
use std::sync::LazyLock;

/// Definition of a single flat vector icon.
#[derive(Debug, Clone)]
pub struct IconDef {
    /// Canonical icon identifier (e.g. `rust`, `postgres`, `user`).
    pub name: &'static str,
    /// Default brand / theme color for the icon.
    pub default_color: &'static str,
    /// SVG inner markup for 24x24 coordinate system.
    pub svg_body: &'static str,
}

// ---------------------------------------------------------------------------
// Icon Registry
// ---------------------------------------------------------------------------

static ICONS: LazyLock<HashMap<&'static str, IconDef>> = LazyLock::new(|| {
    let mut m = HashMap::new();

    macro_rules! vendored {
        ($key:literal, $color:literal, $file:literal) => {
            m.insert(
                $key,
                IconDef {
                    name: $key,
                    default_color: $color,
                    svg_body: include_str!(concat!("../assets/", $file)),
                },
            );
        };
    }

    // --- Programming Languages & Runtimes (Simple Icons, CC0) ---------------
    vendored!("rust", "#000000", "rust.svg");
    vendored!("python", "#3776ab", "python.svg");
    vendored!("go", "#00add8", "go.svg");
    vendored!("typescript", "#3178c6", "typescript.svg");
    vendored!("javascript", "#f7df1e", "javascript.svg");
    vendored!("kotlin", "#7f52ff", "kotlin.svg");
    vendored!("cpp", "#00599c", "cpp.svg");
    vendored!("ruby", "#cc342d", "ruby.svg");
    vendored!("swift", "#f05138", "swift.svg");
    vendored!("node", "#5fa04e", "node.svg");
    vendored!("php", "#777bb4", "php.svg");
    vendored!("scala", "#dc322f", "scala.svg");
    vendored!("elixir", "#4b275f", "elixir.svg");
    vendored!("dart", "#0175c2", "dart.svg");
    vendored!("r", "#276dc3", "r.svg");
    vendored!("perl", "#0073a1", "perl.svg");
    vendored!("lua", "#000080", "lua.svg");

    // Java and C#/.NET have no CC0-licensed mark available (Simple Icons carries neither
    // the Oracle "Java coffee cup" nor a C# glyph), so these stay hand-drawn.
    m.insert(
        "java",
        IconDef {
            name: "java",
            default_color: "#ea2d2e",
            svg_body: r##"<path fill="#5382a1" d="M5.5 17.5c2 1 7 1 9 0 .5-.3.8-.7.8-1.2H4.7c0 .5.3.9.8 1.2zM4 19c2.5 1.2 8.5 1.2 11 0l-.5-1c-2.3 1-7.7 1-10 0L4 19zm13-8.5h-1.5v4.5H17c1.4 0 2.5-1 2.5-2.2s-1.1-2.3-2.5-2.3zm0 3.3h-.5V12h.5c.7 0 1.2.4 1.2 1s-.5 1-1.2 1zM7 10.5h7.5v5H7z"/><path fill="#ea2d2e" d="M9.5 3c-.8 1.5-.2 2.5.8 3.5 1 1 .5 2-.5 3 .8-.8 1.2-1.8.8-2.7-.4-.9-1.2-1.5-.8-2.8.2-.5 0-.8-.3-1zm3 1c-.6 1.2-.2 2 .6 2.8.8.8.4 1.6-.4 2.4.6-.6 1-1.4.6-2.1-.3-.7-1-1.2-.6-2.2.1-.4 0-.7-.2-.9z"/>"##,
        },
    );
    m.insert(
        "csharp",
        IconDef {
            name: "csharp",
            default_color: "#512bd4",
            svg_body: r##"<path fill="#512bd4" d="M12 2 3 7v10l9 5 9-5V7l-9-5zm-2 13.5a4 4 0 1 1 0-8 4 4 0 0 1 2.8 1.2l-1.2 1.3a2.3 2.3 0 1 0 0 3l1.2 1.3A4 4 0 0 1 10 15.5zm4-2h.8l.2-1h-.8l.2-1h-1l-.2 1h-.8l.2-1h-1l-.2 1h-.8l-.2 1h.8l-.2 1h-.8l-.2 1h.8l-.2 1h1l.2-1h.8l-.2 1h1l.2-1h.8l.2-1h-.8l.2-1zm-1.8 0h.8l-.2 1h-.8l.2-1z"/>"##,
        },
    );

    // --- Cloud & Deployment Platforms ----------------------------------------
    vendored!("googlecloud", "#4285f4", "googlecloud.svg");
    vendored!("cloudflare", "#f38020", "cloudflare.svg");
    vendored!("vercel", "#000000", "vercel.svg");
    vendored!("netlify", "#00c7b7", "netlify.svg");
    vendored!("digitalocean", "#0080ff", "digitalocean.svg");

    // AWS: Simple Icons carries no Amazon-family marks (removed at Amazon's request), so
    // this stays hand-drawn — a generic dark-cloud-tile approximation, not a copy of the
    // registered trademark.
    m.insert(
        "aws",
        IconDef {
            name: "aws",
            default_color: "#ff9900",
            svg_body: r##"<path fill="#232f3e" d="M2 3h20v18H2z"/><path fill="#ff9900" d="M6 15.5c3.2 2 8.8 2 12 0l-.8-1.2c-2.8 1.8-7.6 1.8-10.4 0L6 15.5zm11.2-.2l1.6 1.2.2-2-1.8.8z"/>"##,
        },
    );

    // --- Database & Data Store Engines ---------------------------------------
    vendored!("postgres", "#4169e1", "postgres.svg");
    vendored!("mysql", "#4479a1", "mysql.svg");
    vendored!("redis", "#ff4438", "redis.svg");
    vendored!("mongodb", "#47a248", "mongodb.svg");
    vendored!("cassandra", "#1287b1", "cassandra.svg");
    vendored!("sqlite", "#003b57", "sqlite.svg");
    vendored!("elasticsearch", "#005571", "elasticsearch.svg");
    vendored!("kafka", "#231f20", "kafka.svg");
    vendored!("rabbitmq", "#ff6600", "rabbitmq.svg");
    vendored!("mariadb", "#003545", "mariadb.svg");
    vendored!("neo4j", "#4581c3", "neo4j.svg");
    vendored!("influxdb", "#22adf6", "influxdb.svg");
    vendored!("cockroachdb", "#6933ff", "cockroachdb.svg");

    // DynamoDB: same Amazon-family removal as AWS above, stays hand-drawn.
    m.insert(
        "dynamodb",
        IconDef {
            name: "dynamodb",
            default_color: "#4053d6",
            svg_body: r##"<path fill="#4053d6" d="M12 2C6.5 2 2 4.2 2 7v10c0 2.8 4.5 5 10 5s10-2.2 10-5V7c0-2.8-4.5-5-10-5zm0 2.5c4.7 0 8 1.6 8 2.5s-3.3 2.5-8 2.5-8-1.6-8-2.5 3.3-2.5 8-2.5zM4 10.2c1.8 1.1 4.7 1.8 8 1.8s6.2-.7 8-1.8V12c0 .9-3.3 2.5-8 2.5S4 12.9 4 12v-1.8zm0 5c1.8 1.1 4.7 1.8 8 1.8s6.2-.7 8-1.8V17c0 .9-3.3 2.5-8 2.5S4 17.9 4 17v-1.8z"/>"##,
        },
    );

    // --- Infra, CI/CD & Observability Tooling --------------------------------
    vendored!("docker", "#2496ed", "docker.svg");
    vendored!("kubernetes", "#326ce5", "kubernetes.svg");
    vendored!("nginx", "#009639", "nginx.svg");
    vendored!("terraform", "#844fba", "terraform.svg");
    vendored!("ansible", "#ee0000", "ansible.svg");
    vendored!("grafana", "#f46800", "grafana.svg");
    vendored!("prometheus", "#e6522c", "prometheus.svg");
    vendored!("jenkins", "#d24939", "jenkins.svg");
    vendored!("githubactions", "#2088ff", "githubactions.svg");
    vendored!("gitlab", "#fc6d26", "gitlab.svg");
    vendored!("github", "#181717", "github.svg");

    // --- Web / Backend Frameworks ---------------------------------------------
    vendored!("react", "#61dafb", "react.svg");
    vendored!("vuejs", "#4fc08d", "vuejs.svg");
    vendored!("angular", "#0f0f11", "angular.svg");
    vendored!("django", "#092e20", "django.svg");
    vendored!("flask", "#3babc3", "flask.svg");
    vendored!("fastapi", "#009688", "fastapi.svg");
    vendored!("spring", "#000000", "spring.svg");
    vendored!("express", "#0a0a0a", "express.svg");
    vendored!("nextjs", "#000000", "nextjs.svg");
    vendored!("dotnet", "#512bd4", "dotnet.svg");
    vendored!("laravel", "#ff2d20", "laravel.svg");
    vendored!("rails", "#d30001", "rails.svg");

    // --- API & Auth ------------------------------------------------------------
    vendored!("graphql", "#e10098", "graphql.svg");
    vendored!("apollographql", "#311c87", "apollographql.svg");
    vendored!("stripe", "#635bff", "stripe.svg");
    vendored!("auth0", "#eb5424", "auth0.svg");
    vendored!("okta", "#007dc1", "okta.svg");

    // --- Generic (non-brand) shapes -------------------------------------------

    // Generic DB / Table Cylinder: 3D data storage cylinder
    m.insert(
        "database",
        IconDef {
            name: "database",
            default_color: "#0284c7",
            svg_body: r##"<ellipse cx="12" cy="5.5" rx="8" ry="3" fill="#0284c7"/><path fill="#0284c7" d="M4 5.5v13c0 1.7 3.6 3 8 3s8-1.3 8-3v-13H4zm8 14.5c-3.5 0-6.5-.9-7.5-2V9.8c1.8 1.1 4.5 1.7 7.5 1.7s5.7-.6 7.5-1.7V17c-1 1.1-4 2-7.5 2z"/>"##,
        },
    );

    m.insert("table", m.get("database").unwrap().clone());

    // User / Client / Person: Clean flat avatar
    m.insert(
        "user",
        IconDef {
            name: "user",
            default_color: "#64748b",
            svg_body: r##"<circle cx="12" cy="7" r="4" fill="#64748b"/><path fill="#64748b" d="M4 21v-2a6 6 0 0 1 6-6h4a6 6 0 0 1 6 6v2H4z"/>"##,
        },
    );

    m
});

// ---------------------------------------------------------------------------
// Detection & Lookup Helpers
// ---------------------------------------------------------------------------

/// Retrieve an icon definition by key.
pub fn get_icon(key: &str) -> Option<&'static IconDef> {
    let lower = key.trim().to_ascii_lowercase();
    ICONS.get(lower.as_str())
}

/// Maps a cloud provider hint (`aws`, `gcp`, `azure`, ...) to an icon key, when one is
/// available. Azure has no entry — Simple Icons carries no Microsoft-family marks (see
/// `assets/THIRD_PARTY_LICENSES.md`) and no hand-drawn Azure approximation exists (yet).
pub fn detect_provider(provider: &str) -> Option<&'static str> {
    match provider.trim().to_ascii_lowercase().as_str() {
        "aws" | "amazon" | "amazon_web_services" => Some("aws"),
        "gcp" | "google" | "googlecloud" | "google_cloud" | "google cloud" => Some("googlecloud"),
        _ => None,
    }
}

/// Detect a programming language key from explicit fields or textual cues.
pub fn detect_language(
    explicit_lang: Option<&str>,
    tech: Option<&str>,
    label: &str,
    metadata: Option<&str>,
) -> Option<&'static str> {
    if let Some(l) = explicit_lang {
        let clean = l.trim().to_ascii_lowercase();
        let key = match clean.as_str() {
            "rs" | "rust" => "rust",
            "py" | "python" | "python3" => "python",
            "go" | "golang" => "go",
            "ts" | "typescript" => "typescript",
            "js" | "javascript" | "node" | "nodejs" => "javascript",
            "java" | "jvm" => "java",
            "kt" | "kotlin" => "kotlin",
            "cs" | "c#" | "csharp" | "dotnet" | ".net" => "csharp",
            "cpp" | "c++" => "cpp",
            "rb" | "ruby" => "ruby",
            "swift" => "swift",
            "php" => "php",
            "scala" => "scala",
            "ex" | "exs" | "elixir" => "elixir",
            "dart" => "dart",
            "r" => "r",
            "pl" | "perl" => "perl",
            "lua" => "lua",
            _ => "",
        };
        if !key.is_empty() {
            return Some(key);
        }
    }

    // Inspect label, tech, metadata for bracketed or prominent tags
    fn has_word(text: &str, word: &str) -> bool {
        text.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
            .any(|w| w.eq_ignore_ascii_case(word))
    }

    let haystacks = [tech.unwrap_or(""), label, metadata.unwrap_or("")];
    for hay in haystacks {
        let lower = hay.to_ascii_lowercase();
        if lower.contains("rust")
            || lower.contains("[rust]")
            || lower.contains("tokio")
            || lower.contains("axum")
        {
            return Some("rust");
        }
        if lower.contains("python")
            || lower.contains("[python]")
            || lower.contains("fastapi")
            || lower.contains("django")
            || lower.contains("flask")
        {
            return Some("python");
        }
        if lower.contains("golang")
            || lower.contains("[go]")
            || has_word(&lower, "gin")
            || lower.contains("gorilla")
        {
            return Some("go");
        }
        if lower.contains("typescript") || lower.contains("[ts]") || lower.contains("[typescript]")
        {
            return Some("typescript");
        }
        if lower.contains("javascript")
            || lower.contains("[js]")
            || lower.contains("nodejs")
            || lower.contains("node.js")
        {
            return Some("node");
        }
        if lower.contains("[java]") || has_word(&lower, "java") || lower.contains("spring") {
            return Some("java");
        }
        if lower.contains("kotlin") || lower.contains("[kotlin]") {
            return Some("kotlin");
        }
        let is_csharp_dotnet = lower.contains("c#")
            || lower.contains("[c#]")
            || lower.contains("csharp")
            || lower.contains("dotnet")
            || lower.contains("[.net]")
            || lower.split_whitespace().any(|w| {
                let trimmed =
                    w.trim_matches(|c: char| !c.is_alphanumeric() && c != '.' && c != '#');
                trimmed == ".net"
                    || trimmed == "asp.net"
                    || trimmed.starts_with(".net-")
                    || trimmed.starts_with(".net/")
            });
        if is_csharp_dotnet {
            return Some("csharp");
        }
        if lower.contains("c++") || lower.contains("cpp") {
            return Some("cpp");
        }
        if has_word(&lower, "ruby") || lower.contains("rails") {
            return Some("ruby");
        }
        if has_word(&lower, "swift") {
            return Some("swift");
        }
        if has_word(&lower, "php") || lower.contains("laravel") {
            return Some("php");
        }
        if lower.contains("scala") || lower.contains("akka") {
            return Some("scala");
        }
        if lower.contains("elixir") || lower.contains("phoenix") {
            return Some("elixir");
        }
        if lower.contains("flutter") || has_word(&lower, "dart") {
            return Some("dart");
        }
        if has_word(&lower, "perl") {
            return Some("perl");
        }
        if has_word(&lower, "lua") {
            return Some("lua");
        }
    }

    None
}

/// Detect a database engine key from explicit fields or textual cues.
pub fn detect_database(
    explicit_db: Option<&str>,
    tech: Option<&str>,
    label: &str,
    node_type: &str,
) -> Option<&'static str> {
    if let Some(db) = explicit_db {
        let clean = db.trim().to_ascii_lowercase();
        let key = match clean.as_str() {
            "postgres" | "postgresql" | "pg" | "psql" => "postgres",
            "mysql" => "mysql",
            "redis" => "redis",
            "mongodb" | "mongo" => "mongodb",
            "dynamodb" | "dynamo" => "dynamodb",
            "kafka" => "kafka",
            "cassandra" => "cassandra",
            "sqlite" => "sqlite",
            "elastic" | "elasticsearch" => "elasticsearch",
            "rabbitmq" | "rabbit" | "amqp" => "rabbitmq",
            "mariadb" => "mariadb",
            "neo4j" => "neo4j",
            "influxdb" | "influx" => "influxdb",
            "cockroachdb" | "cockroach" => "cockroachdb",
            "table" | "entity" => "table",
            "db" | "database" => "database",
            _ => "",
        };
        if !key.is_empty() {
            return Some(key);
        }
    }

    let haystacks = [tech.unwrap_or(""), label, node_type];
    for hay in haystacks {
        let lower = hay.to_ascii_lowercase();
        if lower.contains("postgres") || lower.contains("psql") {
            return Some("postgres");
        }
        if lower.contains("mysql") {
            return Some("mysql");
        }
        if lower.contains("redis") {
            return Some("redis");
        }
        if lower.contains("mongodb") || lower.contains("mongo") {
            return Some("mongodb");
        }
        if lower.contains("dynamodb") || lower.contains("dynamo") {
            return Some("dynamodb");
        }
        if lower.contains("kafka") {
            return Some("kafka");
        }
        if lower.contains("cassandra") {
            return Some("cassandra");
        }
        if lower.contains("sqlite") {
            return Some("sqlite");
        }
        if lower.contains("elasticsearch") || lower.contains("elastic") {
            return Some("elasticsearch");
        }
        if lower.contains("rabbitmq") || lower.contains("rabbit") {
            return Some("rabbitmq");
        }
        if lower.contains("mariadb") {
            return Some("mariadb");
        }
        if lower.contains("neo4j") {
            return Some("neo4j");
        }
        if lower.contains("influxdb") || lower.contains("influx") {
            return Some("influxdb");
        }
        if lower.contains("cockroach") {
            return Some("cockroachdb");
        }
    }

    if matches!(
        node_type.to_ascii_lowercase().as_str(),
        "table" | "entity" | "record"
    ) {
        return Some("table");
    }
    if matches!(
        node_type.to_ascii_lowercase().as_str(),
        "database" | "db" | "storage"
    ) {
        return Some("database");
    }

    None
}

/// Generates a self-contained RFC 2397 Data URI (`data:image/svg+xml;base64,...`)
/// for embedding in draw.io `<img>` elements.
pub fn icon_as_data_uri(key: &str) -> Option<String> {
    let icon = get_icon(key)?;
    let svg_xml = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="24" height="24">{}</svg>"##,
        icon.svg_body
    );
    let b64 = base64::engine::general_purpose::STANDARD.encode(svg_xml.as_bytes());
    Some(format!("data:image/svg+xml;base64,{b64}"))
}

/// Renders the vector icon as an inline SVG `<g>` transformed to `(x, y)` with given size.
pub fn render_icon_svg(key: &str, x: f64, y: f64, size: f64) -> Option<String> {
    let icon = get_icon(key)?;
    let scale = size / 24.0;
    Some(format!(
        r##"<g class="node-tech-icon" transform="translate({x:.1}, {y:.1}) scale({scale:.4})">{}</g>"##,
        icon.svg_body
    ))
}

/// Renders the vector icon centered at a vertex or edge `(vertex_x, vertex_y)` without a wrapper tile.
///
/// Strips away the background box tile, stroke border, and drop shadow so the raw vector icon alone is rendered cleanly.
pub fn render_icon_badge_svg(
    key: &str,
    vertex_x: f64,
    vertex_y: f64,
    _is_dark: bool,
) -> Option<String> {
    let icon = get_icon(key)?;
    let icon_size = 22.0;
    let icon_x = vertex_x - icon_size / 2.0;
    let icon_y = vertex_y - icon_size / 2.0;
    let scale = icon_size / 24.0;

    Some(format!(
        r##"<g class="node-icon-badge">
  <g class="node-tech-icon" transform="translate({icon_x:.1}, {icon_y:.1}) scale({scale:.4})">
    {}
  </g>
</g>"##,
        icon.svg_body
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_icon_lookup_and_data_uri() {
        let rust_uri = icon_as_data_uri("rust").expect("rust icon must exist");
        assert!(rust_uri.starts_with("data:image/svg+xml;base64,"));

        let pg_uri = icon_as_data_uri("postgres").expect("postgres icon must exist");
        assert!(pg_uri.starts_with("data:image/svg+xml;base64,"));

        let user_uri = icon_as_data_uri("user").expect("user icon must exist");
        assert!(user_uri.starts_with("data:image/svg+xml;base64,"));
    }

    #[test]
    fn test_detect_language_and_database() {
        assert_eq!(
            detect_language(Some("rust"), None, "Auth Service", None),
            Some("rust")
        );
        assert_eq!(
            detect_language(None, Some("FastAPI / Python 3.12"), "Order Service", None),
            Some("python")
        );
        assert_eq!(
            detect_language(None, None, "Billing Svc [Go]", None),
            Some("go")
        );

        assert_eq!(
            detect_database(Some("postgres"), None, "User Database", "database"),
            Some("postgres")
        );
        assert_eq!(
            detect_database(None, Some("Redis Cache"), "Sessions", "cache"),
            Some("redis")
        );
        assert_eq!(detect_database(None, None, "users", "table"), Some("table"));
    }

    #[test]
    fn test_render_icon_svg() {
        // Uses the hand-drawn "java" icon (stable color, not tied to a vendored dataset).
        let svg_g = render_icon_svg("java", 10.0, 20.0, 16.0).expect("svg rendering");
        assert!(svg_g.contains("translate(10.0, 20.0)"));
        assert!(svg_g.contains("#5382a1"));
    }

    #[test]
    fn test_render_icon_badge_svg() {
        let badge = render_icon_badge_svg("java", 100.0, 200.0, false).expect("badge rendering");
        assert!(badge.contains("node-icon-badge"));
        assert!(badge.contains("translate(89.0, 189.0)"));
        assert!(!badge.contains("<rect"));
    }

    #[test]
    fn test_detect_language_no_false_positive_domain_names() {
        // "diagrams.net" or any URL should not trigger C# detection
        let res = detect_language(
            None,
            None,
            "output.drawio",
            Some("Uncompressed mxfile. Open in draw.io desktop or diagrams.net"),
        );
        assert_eq!(res, None);

        // Standalone .net, [.net], or asp.net should trigger C#
        assert_eq!(
            detect_language(None, Some(".NET Core"), "Backend", None),
            Some("csharp")
        );
        assert_eq!(
            detect_language(None, None, "Backend [.NET]", None),
            Some("csharp")
        );
        assert_eq!(
            detect_language(None, Some("ASP.NET"), "API", None),
            Some("csharp")
        );

        // "engine" should not trigger "gin" (Go)
        assert_eq!(
            detect_language(None, None, "Hierarchical Layout Engine", None),
            None
        );
        // But standalone "gin" or "Gin Gonic" should
        assert_eq!(
            detect_language(None, Some("Gin Gonic"), "Web Server", None),
            Some("go")
        );
    }

    // -----------------------------------------------------------------------
    // Phase B: vendored Simple Icons coverage
    // -----------------------------------------------------------------------

    #[test]
    fn test_vendored_icons_present_and_well_formed() {
        let keys = [
            "rust",
            "python",
            "go",
            "typescript",
            "javascript",
            "kotlin",
            "cpp",
            "ruby",
            "swift",
            "node",
            "php",
            "scala",
            "elixir",
            "dart",
            "r",
            "perl",
            "lua",
            "googlecloud",
            "cloudflare",
            "vercel",
            "netlify",
            "digitalocean",
            "postgres",
            "mysql",
            "redis",
            "mongodb",
            "cassandra",
            "sqlite",
            "elasticsearch",
            "kafka",
            "rabbitmq",
            "mariadb",
            "neo4j",
            "influxdb",
            "cockroachdb",
            "docker",
            "kubernetes",
            "nginx",
            "terraform",
            "ansible",
            "grafana",
            "prometheus",
            "jenkins",
            "githubactions",
            "gitlab",
            "github",
            "react",
            "vuejs",
            "angular",
            "django",
            "flask",
            "fastapi",
            "spring",
            "express",
            "nextjs",
            "dotnet",
            "laravel",
            "rails",
            "graphql",
            "apollographql",
            "stripe",
            "auth0",
            "okta",
        ];
        for key in keys {
            let icon = get_icon(key).unwrap_or_else(|| panic!("icon '{key}' should be registered"));
            assert!(
                icon.svg_body.contains("<path"),
                "icon '{key}' should contain a <path>"
            );
            assert!(
                icon.svg_body.contains("fill="),
                "icon '{key}' should set a fill color"
            );
        }
    }

    #[test]
    fn test_hand_drawn_icons_kept_for_unavailable_brands() {
        // Brands Simple Icons doesn't carry (Amazon/Microsoft-family, Java, C#) must still work.
        for key in ["java", "csharp", "aws", "dynamodb"] {
            assert!(
                get_icon(key).is_some(),
                "hand-drawn icon '{key}' must still be registered"
            );
        }
    }

    #[test]
    fn test_detect_provider() {
        assert_eq!(detect_provider("aws"), Some("aws"));
        assert_eq!(detect_provider("AWS"), Some("aws"));
        assert_eq!(detect_provider("gcp"), Some("googlecloud"));
        assert_eq!(detect_provider("Google Cloud"), Some("googlecloud"));
        // Azure has no vendored or hand-drawn icon available.
        assert_eq!(detect_provider("azure"), None);
    }

    #[test]
    fn test_detect_language_new_languages() {
        assert_eq!(detect_language(Some("php"), None, "", None), Some("php"));
        assert_eq!(
            detect_language(Some("elixir"), None, "", None),
            Some("elixir")
        );
        assert_eq!(
            detect_language(None, None, "Payments [Scala]", None),
            Some("scala")
        );
        assert_eq!(
            detect_language(None, Some("Flutter"), "Mobile App", None),
            Some("dart")
        );
    }

    #[test]
    fn test_detect_database_new_engines() {
        assert_eq!(
            detect_database(Some("mariadb"), None, "", "database"),
            Some("mariadb")
        );
        assert_eq!(
            detect_database(None, Some("Neo4j Graph"), "Recs", "database"),
            Some("neo4j")
        );
        assert_eq!(
            detect_database(None, None, "events.rabbitmq", "queue"),
            Some("rabbitmq")
        );
    }

    #[test]
    fn test_unknown_icon_key_returns_none() {
        assert!(get_icon("not-a-real-icon-key").is_none());
        assert!(icon_as_data_uri("not-a-real-icon-key").is_none());
        assert!(render_icon_svg("not-a-real-icon-key", 0.0, 0.0, 10.0).is_none());
    }
}
