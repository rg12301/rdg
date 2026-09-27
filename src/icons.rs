//! Built-in flat vector icons for programming languages, database engines, and actors.
//!
//! Provides clean, zero-dependency SVG vector icons (24x24 viewBox) for:
//! - Programming languages & runtimes (Rust, Go, Python, TypeScript, Java, etc.)
//! - Database engines (PostgreSQL, MySQL, Redis, MongoDB, DynamoDB, etc.)
//! - Data storage cylinders and user/actor avatars
//!
//! Icons can be rendered directly into SVG diagrams or base64-encoded for
//! draw.io embedded HTML images.

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
// Pure-Rust Zero-Dependency Base64 Encoder
// ---------------------------------------------------------------------------

/// Encodes raw bytes into standard RFC 4648 Base64 string.
pub fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0];
        let b1 = if chunk.len() > 1 { chunk[1] } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] } else { 0 };
        out.push(TABLE[(b0 >> 2) as usize] as char);
        out.push(TABLE[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(b2 & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Icon Registry
// ---------------------------------------------------------------------------

static ICONS: LazyLock<HashMap<&'static str, IconDef>> = LazyLock::new(|| {
    let mut m = HashMap::new();

    // --- Programming Languages & Runtimes -----------------------------------

    // Rust: Iconic gear with bold 'R'
    m.insert(
        "rust",
        IconDef {
            name: "rust",
            default_color: "#ce422b",
            svg_body: r##"<path fill="#ce422b" d="M12 2a10 10 0 1 0 10 10A10 10 0 0 0 12 2zm0 2a8 8 0 0 1 7.4 5H18a1 1 0 0 0-1 1v1h-1.5A3.5 3.5 0 0 0 12 7.5a3.5 3.5 0 0 0-3.5 3.5H7v-1a1 1 0 0 0-1-1H4.6A8 8 0 0 1 12 4zm-2.5 6h3a1.5 1.5 0 0 1 0 3h-3zm-1.5-1.5v7h1.5v-2.5h1.2l2.3 2.5H16l-2.6-2.8a2.5 2.5 0 0 0 1.6-2.2 2.5 2.5 0 0 0-2.5-2.5H8z"/>"##,
        },
    );

    // Python: Iconic dual snakes (blue & gold)
    m.insert(
        "python",
        IconDef {
            name: "python",
            default_color: "#3776ab",
            svg_body: r##"<path fill="#3776ab" d="M11.9 2c-4 0-3.8 1.7-3.8 1.7v1.8h3.9v.5H6.5S3 5.5 3 9.6s3 4 3 4h1V12a3 3 0 0 1 3-3h3.9V7.1s.4-5.1-5-5.1zm-2 1.3a.7.7 0 1 1 0 1.4.7.7 0 0 1 0-1.4z"/><path fill="#ffd438" d="M12.1 22c4 0 3.8-1.7 3.8-1.7v-1.8H12v-.5h5.5s3.5.5 3.5-3.6-3-4-3-4h-1V12a3 3 0 0 1-3 3H9.6v1.9s-.4 5.1 5 5.1zm2-1.3a.7.7 0 1 1 0-1.4.7.7 0 0 1 0 1.4z"/>"##,
        },
    );

    // Go / Golang: Bold geometric GO badge
    m.insert(
        "go",
        IconDef {
            name: "go",
            default_color: "#00add8",
            svg_body: r##"<rect width="22" height="16" x="1" y="4" rx="3" fill="#00add8"/><path fill="#ffffff" d="M5.5 8h4.5v1.8H7.3v4.4h2.7v-1.8H8.5v-1.4h3.3v4.5H5.5V8zm7.5 0h4.8c1.2 0 2.2 1 2.2 2.2v2.8c0 1.2-1 2.2-2.2 2.2H13V8zm1.8 1.8v3.6h2.8c.3 0 .5-.2.5-.5v-2.6c0-.3-.2-.5-.5-.5h-2.8z"/>"##,
        },
    );

    // TypeScript: TS blue tile with crisp white TS
    m.insert(
        "typescript",
        IconDef {
            name: "typescript",
            default_color: "#3178c6",
            svg_body: r##"<rect width="22" height="22" x="1" y="1" rx="4" fill="#3178c6"/><path fill="#ffffff" d="M4.5 9h6.5v1.8H8.8v6.7H6.7v-6.7H4.5V9zm8 2.2c.7-.6 1.7-.9 2.7-.9 1.9 0 3 1 3 2.6v.2c-.7-.4-1.6-.6-2.5-.6-1.5 0-2.4.7-2.4 1.7 0 1 .8 1.6 2.2 1.6 1 0 1.9-.3 2.7-.8v1.4c-.8.5-1.9.7-2.9.7-2.4 0-3.9-1.3-3.9-3.3 0-1.2.5-2 1.5-2.6zm3.3 2.1c.6 0 1.1.2 1.6.5v-.9c0-.7-.5-1.1-1.4-1.1-.7 0-1.4.2-1.8.6.3.6.9.9 1.6.9z"/>"##,
        },
    );

    // JavaScript: JS yellow tile with black JS
    m.insert(
        "javascript",
        IconDef {
            name: "javascript",
            default_color: "#f7df1e",
            svg_body: r##"<rect width="22" height="22" x="1" y="1" rx="4" fill="#f7df1e"/><path fill="#000000" d="M7 14.5v-5h1.8v4c0 .8.3 1.2 1 1.2.5 0 .9-.3 1-.7l1.5.8c-.5 1-1.4 1.5-2.6 1.5-1.8 0-2.7-1-2.7-2.8zm7.3-1.2c.6.4 1.3.7 2 .7.7 0 1-.3 1-.6 0-.4-.4-.5-1.3-.8-1.5-.4-2.4-.9-2.4-2.1 0-1.4 1.2-2.3 2.8-2.3 1 0 1.8.3 2.4.7l-.8 1.4c-.5-.3-1-.5-1.6-.5-.6 0-.9.2-.9.5 0 .3.3.4 1.2.7 1.6.4 2.5 1 2.5 2.2 0 1.5-1.2 2.4-3 2.4-1.2 0-2.2-.4-2.8-.9l.9-1.5z"/>"##,
        },
    );

    // Java: Classic coffee cup
    m.insert(
        "java",
        IconDef {
            name: "java",
            default_color: "#ea2d2e",
            svg_body: r##"<path fill="#ea2d2e" d="M6 10h9v6a4 4 0 0 1-4 4H9a4 4 0 0 1-4-4v-6h1zm9 1.5h1.5a1.5 1.5 0 0 1 1.5 1.5v1a1.5 1.5 0 0 1-1.5 1.5H15v-5zM4 21h13v1.5H4V21zm5-17c1.5 1 1.5 2.5 0 4 .8-1.2.8-2.8 0-4zm3 0c1.5 1 1.5 2.5 0 4 .8-1.2.8-2.8 0-4z"/>"##,
        },
    );

    // Kotlin: Geometric K
    m.insert(
        "kotlin",
        IconDef {
            name: "kotlin",
            default_color: "#7f52ff",
            svg_body: r##"<path fill="#7f52ff" d="M2 2h20L12 12 22 22H2V2zm0 10 10-10H2v10zm10 0L2 22h10l5-5-5-5z"/>"##,
        },
    );

    // C++: Shield with C++ emblem
    m.insert(
        "cpp",
        IconDef {
            name: "cpp",
            default_color: "#00599c",
            svg_body: r##"<path fill="#00599c" d="M12 2 3 6v6c0 5.5 3.8 10.7 9 12 5.2-1.3 9-6.5 9-12V6l-9-4zm-2 13a3.5 3.5 0 1 1 0-7 3.5 3.5 0 0 1 2.5 1.1l-1.1 1.1a2 2 0 1 0 0 2.6l1.1 1.1A3.5 3.5 0 0 1 10 15zm4-2.5h1v-1h-1v-1h-1v1h-1v1h1v1h1v-1zm4 0h1v-1h-1v-1h-1v1h-1v1h1v1h1v-1z"/>"##,
        },
    );

    // C# / .NET: Purple badge with C#
    m.insert(
        "csharp",
        IconDef {
            name: "csharp",
            default_color: "#512bd4",
            svg_body: r##"<path fill="#512bd4" d="M12 2 3 7v10l9 5 9-5V7l-9-5zm-2 13.5a4 4 0 1 1 0-8 4 4 0 0 1 2.8 1.2l-1.2 1.3a2.3 2.3 0 1 0 0 3l1.2 1.3A4 4 0 0 1 10 15.5zm4-2h.8l.2-1h-.8l.2-1h-1l-.2 1h-.8l.2-1h-1l-.2 1h-.8l-.2 1h.8l-.2 1h-.8l-.2 1h.8l-.2 1h1l.2-1h.8l-.2 1h1l.2-1h.8l.2-1h-.8l.2-1zm-1.8 0h.8l-.2 1h-.8l.2-1z"/>"##,
        },
    );

    // Ruby: Red faceted gemstone
    m.insert(
        "ruby",
        IconDef {
            name: "ruby",
            default_color: "#cc342d",
            svg_body: r##"<path fill="#cc342d" d="M6 3h12l4 6-10 12L2 9l4-6zm.8 1.8L4.1 8.5h3.6l1.3-3.7H6.8zm4 0-1.3 3.7h5l-1.3-3.7h-2.4zm4.4 0 1.3 3.7h3.6l-2.7-3.7h-2.2zM4 10l8 9.5L5.7 10H4zm9.3 0L12 19.5 20 10h-6.7z"/>"##,
        },
    );

    // Node.js: Green hexagon
    m.insert(
        "node",
        IconDef {
            name: "node",
            default_color: "#5fa04e",
            svg_body: r##"<path fill="#5fa04e" d="M12 2 3 7.2v10.4L12 22l9-4.8V7.2L12 2zm0 3.2 6.5 3.8v7.6L12 20.4 5.5 16.6V9L12 5.2zm-2 4.3v4.5l3.5 2 3.5-2V9.5h-1.5v3.6L13.5 14l-2-1.1V9.5H10z"/>"##,
        },
    );

    // Swift: Swift bird in orange
    m.insert(
        "swift",
        IconDef {
            name: "swift",
            default_color: "#f05138",
            svg_body: r##"<path fill="#f05138" d="M21.5 16.5c-.8-1.5-2.2-3.3-4-4.8 2.2 2.6 1.8 4.7 1.8 4.7s-1.8-1.5-3.8-3c-2.4-1.9-5-4.4-6-7.4 2.8 3.5 6 6 8.5 7.5-1.5-1.5-3.2-3.3-4.5-5.5C11.5 5 10 2.5 10 2.5s-.5 3-2 5.5c-1.8 3-4.5 5-5.5 5.5 2.5-.5 5.5-2 7.5-4.5-2 2-4.5 4-7.5 5 3.5 1 7.5.5 10.5-1.5-2 1.5-4.5 2.5-7.5 3 4.5 1.5 9.5 0 12.5-2.5 1.8-1.5 3-3.5 3.5-4z"/>"##,
        },
    );

    // --- Database Engines ---------------------------------------------------

    // PostgreSQL: Stylized elephant head
    m.insert(
        "postgres",
        IconDef {
            name: "postgres",
            default_color: "#336791",
            svg_body: r##"<path fill="#336791" d="M12 2C6.5 2 2 6.5 2 12c0 3.8 2.1 7.1 5.3 8.8.4-.5.8-1.2 1.1-2-1.7-.5-3-1.8-3.4-3.5.8.4 1.8.6 2.8.5-.2-.7-.3-1.4-.3-2.1 0-3.3 2.7-6 6-6s6 2.7 6 6c0 .7-.1 1.4-.3 2.1 1 .1 2-.1 2.8-.5-.4 1.7-1.7 3-3.4 3.5.3.8.7 1.5 1.1 2 3.2-1.7 5.3-5 5.3-8.8 0-5.5-4.5-10-10-10zm-1.5 8c-.6 0-1 .4-1 1s.4 1 1 1 1-.4 1-1-.4-1-1-1zm3 0c-.6 0-1 .4-1 1s.4 1 1 1 1-.4 1-1-.4-1-1-1z"/>"##,
        },
    );

    // MySQL: Leaping dolphin
    m.insert(
        "mysql",
        IconDef {
            name: "mysql",
            default_color: "#00758f",
            svg_body: r##"<path fill="#00758f" d="M19 12c-1.5-3-4-5-7-5.5 1-1.5 2.5-2 4-2-3 0-6 1.5-8 4-2 2.5-3 6-3 9 1.5-1 3.5-1.5 5.5-1.5 3.5 0 6.5 1.5 8.5 4 0-2.5-.5-5.5-2-8zm-8-2c-.6 0-1-.4-1-1s.4-1 1-1 1 .4 1 1-.4 1-1 1z"/>"##,
        },
    );

    // Redis: Isometric stacked memory cube
    m.insert(
        "redis",
        IconDef {
            name: "redis",
            default_color: "#dc382d",
            svg_body: r##"<path fill="#dc382d" d="M12 2 2 7l10 5 10-5-10-5zm0 6L4.5 5 12 3.2 19.5 5 12 8zM2 10l10 5 10-5v2l-10 5-10-5v-2zm0 5l10 5 10-5v2l-10 5-10-5v-2z"/>"##,
        },
    );

    // MongoDB: Symmetric green leaf
    m.insert(
        "mongodb",
        IconDef {
            name: "mongodb",
            default_color: "#47a248",
            svg_body: r##"<path fill="#47a248" d="M12 2s-6 5.5-6 11c0 4.2 3.4 7.6 6 8.8 2.6-1.2 6-4.6 6-8.8 0-5.5-6-11-6-11zm.6 17.5v-7.3c0-.3-.3-.5-.6-.5s-.6.2-.6.5v7.3C9.7 18.5 7.5 15.6 7.5 13c0-3.8 3.5-7.7 4.5-8.8 1 1.1 4.5 5 4.5 8.8 0 2.6-2.2 5.5-3.9 6.5z"/>"##,
        },
    );

    // DynamoDB: AWS multi-ring grid
    m.insert(
        "dynamodb",
        IconDef {
            name: "dynamodb",
            default_color: "#4053d6",
            svg_body: r##"<path fill="#4053d6" d="M12 2C6.5 2 2 4.2 2 7v10c0 2.8 4.5 5 10 5s10-2.2 10-5V7c0-2.8-4.5-5-10-5zm0 2.5c4.7 0 8 1.6 8 2.5s-3.3 2.5-8 2.5-8-1.6-8-2.5 3.3-2.5 8-2.5zM4 10.2c1.8 1.1 4.7 1.8 8 1.8s6.2-.7 8-1.8V12c0 .9-3.3 2.5-8 2.5S4 12.9 4 12v-1.8zm0 5c1.8 1.1 4.7 1.8 8 1.8s6.2-.7 8-1.8V17c0 .9-3.3 2.5-8 2.5S4 17.9 4 17v-1.8z"/>"##,
        },
    );

    // Kafka: Event streaming hub with nodes
    m.insert(
        "kafka",
        IconDef {
            name: "kafka",
            default_color: "#231f20",
            svg_body: r##"<circle cx="12" cy="12" r="3" fill="#231f20"/><circle cx="5" cy="7" r="2.5" fill="#231f20"/><circle cx="5" cy="17" r="2.5" fill="#231f20"/><circle cx="19" cy="7" r="2.5" fill="#231f20"/><circle cx="19" cy="17" r="2.5" fill="#231f20"/><path stroke="#231f20" stroke-width="1.5" d="M7 8l3 3M7 16l3-3M17 8l-3 3M17 16l-3-3"/>"##,
        },
    );

    // Cassandra: Cyclops eye in circle
    m.insert(
        "cassandra",
        IconDef {
            name: "cassandra",
            default_color: "#1287b1",
            svg_body: r##"<path fill="#1287b1" d="M12 2a10 10 0 1 0 10 10A10 10 0 0 0 12 2zm0 15a5 5 0 1 1 5-5 5 5 0 0 1-5 5zm0-8a3 3 0 1 0 3 3 3 3 0 0 0-3-3z"/>"##,
        },
    );

    // SQLite: Feather badge
    m.insert(
        "sqlite",
        IconDef {
            name: "sqlite",
            default_color: "#003b57",
            svg_body: r##"<rect width="22" height="18" x="1" y="3" rx="3" fill="#003b57"/><path fill="#ffffff" d="M4 8h10v1.8H6.5v2.2H12v1.8H6.5v2.4H4V8zm12 0h2v8.2h-2V8z"/>"##,
        },
    );

    // Elasticsearch: Cluster circle
    m.insert(
        "elasticsearch",
        IconDef {
            name: "elasticsearch",
            default_color: "#005571",
            svg_body: r##"<circle cx="12" cy="12" r="9" fill="#005571"/><path fill="#ffffff" d="M7 11h10v2H7zm2-4h6v2H9zm2 8h4v2h-4z"/>"##,
        },
    );

    // Generic DB / Table Cylinder: 3D data storage cylinder
    m.insert(
        "database",
        IconDef {
            name: "database",
            default_color: "#0284c7",
            svg_body: r##"<ellipse cx="12" cy="5" rx="8" ry="3" fill="#0284c7"/><path fill="#0284c7" d="M4 5v14c0 1.7 3.6 3 8 3s8-1.3 8-3V5H4zm8 15c-3.5 0-6.5-.9-7.5-2V9.8c1.8 1.1 4.5 1.7 7.5 1.7s5.7-.6 7.5-1.7V18c-1 1.1-4 2-7.5 2z"/><path fill="#ffffff" opacity=".4" d="M12 9c-3.5 0-6.5-.9-7.5-2 .5-.6 1.8-1.2 3.5-1.5V7c0 .6 1.8 1 4 1s4-.4 4-1v-1.5c1.7.3 3 .9 3.5 1.5-1 1.1-4 2-7.5 2z"/>"##,
        },
    );

    m.insert("table", m.get("database").unwrap().clone());

    // --- Users & Actors -----------------------------------------------------

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
            _ => "",
        };
        if !key.is_empty() {
            return Some(key);
        }
    }

    // Inspect label, tech, metadata for bracketed or prominent tags
    let haystacks = [tech.unwrap_or(""), label, metadata.unwrap_or("")];
    for hay in haystacks {
        let lower = hay.to_ascii_lowercase();
        if lower.contains("rust") || lower.contains("[rust]") || lower.contains("tokio") || lower.contains("axum") {
            return Some("rust");
        }
        if lower.contains("python") || lower.contains("[python]") || lower.contains("fastapi") || lower.contains("django") || lower.contains("flask") {
            return Some("python");
        }
        if lower.contains("golang") || lower.contains("[go]") || lower.contains("gin") || lower.contains("gorilla") {
            return Some("go");
        }
        if lower.contains("typescript") || lower.contains("[ts]") || lower.contains("[typescript]") {
            return Some("typescript");
        }
        if lower.contains("javascript") || lower.contains("[js]") || lower.contains("nodejs") || lower.contains("node.js") {
            return Some("node");
        }
        if lower.contains("java") || lower.contains("[java]") || lower.contains("spring") {
            return Some("java");
        }
        if lower.contains("kotlin") || lower.contains("[kotlin]") {
            return Some("kotlin");
        }
        if lower.contains("c#") || lower.contains("csharp") || lower.contains(".net") {
            return Some("csharp");
        }
        if lower.contains("c++") || lower.contains("cpp") {
            return Some("cpp");
        }
        if lower.contains("ruby") || lower.contains("rails") {
            return Some("ruby");
        }
        if lower.contains("swift") {
            return Some("swift");
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
    }

    if matches!(node_type.to_ascii_lowercase().as_str(), "table" | "entity" | "record") {
        return Some("table");
    }
    if matches!(node_type.to_ascii_lowercase().as_str(), "database" | "db" | "storage") {
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
    let b64 = base64_encode(svg_xml.as_bytes());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base64_encode() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"Rust"), "UnVzdA==");
    }

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
        assert_eq!(
            detect_database(None, None, "users", "table"),
            Some("table")
        );
    }

    #[test]
    fn test_render_icon_svg() {
        let svg_g = render_icon_svg("rust", 10.0, 20.0, 16.0).expect("svg rendering");
        assert!(svg_g.contains("translate(10.0, 20.0)"));
        assert!(svg_g.contains("#ce422b"));
    }
}
