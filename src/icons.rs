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

    // Rust: Official Rust gear with teeth and iconic stylized 'R'
    m.insert(
        "rust",
        IconDef {
            name: "rust",
            default_color: "#ce422b",
            svg_body: r##"<g fill="#ce422b"><path d="M12 2a10 10 0 0 0-2.3.27l-.46 1.48-1.5-.32-.82 1.32-1.3-.84-1.12 1.08-.98-1.2-1.37.73-.59-1.44A10 10 0 0 0 2 12c0 5.16 3.9 9.42 8.92 9.94l.44-1.48 1.5.32.82-1.32 1.3.84 1.12-1.08.98 1.2 1.37-.73.59 1.44A10 10 0 0 0 22 12c0-5.16-3.9-9.42-8.92-9.94l-.44 1.48-1.5-.32-.82 1.32-1.3-.84-1.12 1.08-.98-1.2-1.37.73-.59-1.44A9.95 9.95 0 0 0 12 2zm0 2.2a7.8 7.8 0 1 1 0 15.6 7.8 7.8 0 0 1 0-15.6z"/><path d="M7.8 7.5v9h2.1v-3.2h1.6l2.3 3.2h2.5l-2.7-3.6a3.1 3.1 0 0 0 1.9-2.8c0-1.7-1.4-2.6-3.7-2.6H7.8zm2.1 1.9h1.7c1 0 1.6.4 1.6 1.2 0 .8-.6 1.2-1.6 1.2H9.9V9.4z"/></g>"##,
        },
    );

    // Python: Official dual-snake intertwined logo with eye dots
    m.insert(
        "python",
        IconDef {
            name: "python",
            default_color: "#3776ab",
            svg_body: r##"<path fill="#3776ab" d="M11.9 2c-4 0-3.8 1.7-3.8 1.7v1.8h3.9v.5H6.5S3 5.5 3 9.6s3 4 3 4h1V12a3 3 0 0 1 3-3h3.9V7.1s.4-5.1-5-5.1zm-2 1.3a.75.75 0 1 1 0 1.5.75.75 0 0 1 0-1.5z"/><path fill="#ffd438" d="M12.1 22c4 0 3.8-1.7 3.8-1.7v-1.8H12v-.5h5.5s3.5.5 3.5-3.6-3-4-3-4h-1V12a3 3 0 0 1-3 3H9.6v1.9s-.4 5.1 5 5.1zm2-1.3a.75.75 0 1 1 0-1.5.75.75 0 0 1 0 1.5z"/>"##,
        },
    );

    // Go / Golang: Official Go logo with speed lines
    m.insert(
        "go",
        IconDef {
            name: "go",
            default_color: "#00add8",
            svg_body: r##"<g fill="#00add8"><path d="M1.5 10.5h4.2v1.5H3.2v2.8h2v-1.2H4v-1.3h2.7v3.7H1.5v-5.5zm6.5 0h4.5c1.4 0 2.5 1.1 2.5 2.7v.2c0 1.6-1.1 2.7-2.5 2.7H8v-5.6zm1.8 1.5v2.6h2.5c.6 0 1-.4 1-1.1v-.4c0-.7-.4-1.1-1-1.1H9.8zM16 11h7v1.4h-7V11zm-2 2.5h8v1.4h-8v-1.4zm3 2.5h5v1.4h-5V16z"/></g>"##,
        },
    );

    // TypeScript: Official TS blue tile with crisp white TS
    m.insert(
        "typescript",
        IconDef {
            name: "typescript",
            default_color: "#3178c6",
            svg_body: r##"<rect width="22" height="22" x="1" y="1" rx="4" fill="#3178c6"/><path fill="#ffffff" d="M4 8.5h6.5v1.8H8.3v6.7H6.2v-6.7H4V8.5zm8 2.2c.7-.6 1.7-.9 2.7-.9 1.9 0 3 1 3 2.6v.2c-.7-.4-1.6-.6-2.5-.6-1.5 0-2.4.7-2.4 1.7 0 1 .8 1.6 2.2 1.6 1 0 1.9-.3 2.7-.8v1.4c-.8.5-1.9.7-2.9.7-2.4 0-3.9-1.3-3.9-3.3 0-1.2.5-2 1.5-2.6zm3.3 2.1c.6 0 1.1.2 1.6.5v-.9c0-.7-.5-1.1-1.4-1.1-.7 0-1.4.2-1.8.6.3.6.9.9 1.6.9z"/>"##,
        },
    );

    // JavaScript: Official JS yellow tile with black JS
    m.insert(
        "javascript",
        IconDef {
            name: "javascript",
            default_color: "#f7df1e",
            svg_body: r##"<rect width="22" height="22" x="1" y="1" rx="4" fill="#f7df1e"/><path fill="#000000" d="M6.5 14.8v-5h1.8v4.2c0 .8.4 1.1 1 1.1.5 0 .8-.3 1-.7l1.5.7c-.5 1-1.4 1.5-2.5 1.5-1.8 0-2.8-1-2.8-2.8zm7.3-1.1c.6.4 1.3.7 2 .7.7 0 1-.3 1-.6 0-.4-.4-.5-1.3-.8-1.5-.4-2.4-.9-2.4-2.1 0-1.4 1.2-2.3 2.8-2.3 1 0 1.8.3 2.4.7l-.8 1.4c-.5-.3-1-.5-1.6-.5-.6 0-.9.2-.9.5 0 .3.3.4 1.2.7 1.6.4 2.5 1 2.5 2.2 0 1.5-1.2 2.4-3 2.4-1.2 0-2.2-.4-2.8-.9l.9-1.4z"/>"##,
        },
    );

    // Java: Iconic coffee cup with curved steam waves
    m.insert(
        "java",
        IconDef {
            name: "java",
            default_color: "#ea2d2e",
            svg_body: r##"<path fill="#5382a1" d="M5.5 17.5c2 1 7 1 9 0 .5-.3.8-.7.8-1.2H4.7c0 .5.3.9.8 1.2zM4 19c2.5 1.2 8.5 1.2 11 0l-.5-1c-2.3 1-7.7 1-10 0L4 19zm13-8.5h-1.5v4.5H17c1.4 0 2.5-1 2.5-2.2s-1.1-2.3-2.5-2.3zm0 3.3h-.5V12h.5c.7 0 1.2.4 1.2 1s-.5 1-1.2 1zM7 10.5h7.5v5H7z"/><path fill="#ea2d2e" d="M9.5 3c-.8 1.5-.2 2.5.8 3.5 1 1 .5 2-.5 3 .8-.8 1.2-1.8.8-2.7-.4-.9-1.2-1.5-.8-2.8.2-.5 0-.8-.3-1zm3 1c-.6 1.2-.2 2 .6 2.8.8.8.4 1.6-.4 2.4.6-.6 1-1.4.6-2.1-.3-.7-1-1.2-.6-2.2.1-.4 0-.7-.2-.9z"/>"##,
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

    // C++: Hexagonal badge with sharp C++
    m.insert(
        "cpp",
        IconDef {
            name: "cpp",
            default_color: "#00599c",
            svg_body: r##"<path fill="#00599c" d="M12 2 3 7v10l9 5 9-5V7l-9-5zm-2 13.5a4 4 0 1 1 0-8 4 4 0 0 1 2.8 1.2l-1.2 1.3a2.3 2.3 0 1 0 0 3l1.2 1.3A4 4 0 0 1 10 15.5zm5-3.5h1.2v-1.2h1.2v1.2h1.2v1.2h-1.2v1.2h-1.2v-1.2H15V12zm4 0h1.2v-1.2h1.2v1.2h1.2v1.2h-1.2v1.2h-1.2v-1.2H19V12z"/>"##,
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

    // Node.js: Green hexagon with 'node' monogram
    m.insert(
        "node",
        IconDef {
            name: "node",
            default_color: "#5fa04e",
            svg_body: r##"<path fill="#5fa04e" d="M12 2 3 7.2v10.4L12 22l9-4.8V7.2L12 2zm0 3.2 6.5 3.8v7.6L12 20.4 5.5 16.6V9L12 5.2zm-2 4.3v4.5l3.5 2 3.5-2V9.5h-1.5v3.6L13.5 14l-2-1.1V9.5H10z"/>"##,
        },
    );

    // Swift: Flying swift bird in official orange
    m.insert(
        "swift",
        IconDef {
            name: "swift",
            default_color: "#f05138",
            svg_body: r##"<path fill="#f05138" d="M21.5 16.5c-.8-1.5-2.2-3.3-4-4.8 2.2 2.6 1.8 4.7 1.8 4.7s-1.8-1.5-3.8-3c-2.4-1.9-5-4.4-6-7.4 2.8 3.5 6 6 8.5 7.5-1.5-1.5-3.2-3.3-4.5-5.5C11.5 5 10 2.5 10 2.5s-.5 3-2 5.5c-1.8 3-4.5 5-5.5 5.5 2.5-.5 5.5-2 7.5-4.5-2 2-4.5 4-7.5 5 3.5 1 7.5.5 10.5-1.5-2 1.5-4.5 2.5-7.5 3 4.5 1.5 9.5 0 12.5-2.5 1.8-1.5 3-3.5 3.5-4z"/>"##,
        },
    );

    // Docker: Whale with shipping container blocks
    m.insert(
        "docker",
        IconDef {
            name: "docker",
            default_color: "#2496ed",
            svg_body: r##"<path fill="#2496ed" d="M22.5 11c-.3 0-1.3.1-2 .6-.6-.7-1.5-1.1-2.5-1.1-.3 0-.6.1-.9.2C16.3 9.4 14.8 9 13 9H3v4c0 3.5 2.5 6.5 6 7 4.5.6 9-.8 11.5-3.5 1.5-1.6 2-3.8 2-5.5zM6 8h2v2H6V8zm3 0h2v2H9V8zm3 0h2v2h-2V8zm-6-3h2v2H6V5zm3 0h2v2H9V5zm3 0h2v2h-2V5zm-6-3h2v2H6V2zm3 0h2v2H9V2z"/>"##,
        },
    );

    // Kubernetes: 7-spoked ship helm
    m.insert(
        "kubernetes",
        IconDef {
            name: "kubernetes",
            default_color: "#326ce5",
            svg_body: r##"<path fill="#326ce5" d="M12 2a10 10 0 1 0 10 10A10 10 0 0 0 12 2zm0 2.2a7.8 7.8 0 0 1 6.8 4l-2.4 1.4a5 5 0 0 0-3.4-1.6V5.2a7.8 7.8 0 0 1-1-.02zm-2 1v2.8a5 5 0 0 0-3.4 1.6L4.2 8.2a7.8 7.8 0 0 1 5.8-3zm-6.2 5 2.5 1.4a5 5 0 0 0 0 3.8L3.8 15.8a7.8 7.8 0 0 1 0-7.6zm2.8 6.4 2.4-1.4a5 5 0 0 0 3.4 1.6v2.8a7.8 7.8 0 0 1-5.8-3zm6.4 3v-2.8a5 5 0 0 0 3.4-1.6l2.4 1.4a7.8 7.8 0 0 1-5.8 3zm4.8-4.4-2.5-1.4a5 5 0 0 0 0-3.8l2.5-1.4a7.8 7.8 0 0 1 0 7.6z"/><circle cx="12" cy="12" r="2.5" fill="#ffffff"/>"##,
        },
    );

    // AWS: Dark cloud tile with orange smile arrow
    m.insert(
        "aws",
        IconDef {
            name: "aws",
            default_color: "#ff9900",
            svg_body: r##"<path fill="#232f3e" d="M2 3h20v18H2z"/><path fill="#ff9900" d="M6 15.5c3.2 2 8.8 2 12 0l-.8-1.2c-2.8 1.8-7.6 1.8-10.4 0L6 15.5zm11.2-.2l1.6 1.2.2-2-1.8.8z"/>"##,
        },
    );

    // --- Database Engines ---------------------------------------------------

    // PostgreSQL: Slonik elephant silhouette with eye
    m.insert(
        "postgres",
        IconDef {
            name: "postgres",
            default_color: "#336791",
            svg_body: r##"<path fill="#336791" d="M12 2C6.8 2 2.5 5.8 2.1 10.8c-.3 3.8 1.4 7.2 4.1 9.2.4-.6.8-1.3 1.1-2.1-1.6-.6-2.7-2-3-3.7.8.4 1.8.6 2.8.5-.2-.8-.3-1.6-.3-2.4 0-3.6 2.8-6.5 6.2-6.5s6.2 2.9 6.2 6.5c0 .8-.1 1.6-.3 2.4 1 .1 2-.1 2.8-.5-.3 1.7-1.4 3.1-3 3.7.3.8.7 1.5 1.1 2.1 2.7-2 4.4-5.4 4.1-9.2C21.5 5.8 17.2 2 12 2zm-1.8 8.5c-.7 0-1.2.5-1.2 1.2s.5 1.2 1.2 1.2 1.2-.5 1.2-1.2-.5-1.2-1.2-1.2zm3.6 0c-.7 0-1.2.5-1.2 1.2s.5 1.2 1.2 1.2 1.2-.5 1.2-1.2-.5-1.2-1.2-1.2z"/><circle cx="10.2" cy="11.7" r="1.1" fill="#ffffff"/><circle cx="13.8" cy="11.7" r="1.1" fill="#ffffff"/>"##,
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

    // Redis: Official stacked 3D rhombus layers
    m.insert(
        "redis",
        IconDef {
            name: "redis",
            default_color: "#dc382d",
            svg_body: r##"<path fill="#dc382d" d="M12 2 2.5 6.8 12 11.5l9.5-4.7L12 2zm0 5.2L5 4.8l7-1.8 7 1.8-7 2.4zM2.5 9.5l9.5 4.8 9.5-4.8v2.2l-9.5 4.8-9.5-4.8V9.5zm0 5l9.5 4.8 9.5-4.8v2.2L12 21.5 2.5 16.7v-2.2z"/>"##,
        },
    );

    // MongoDB: Official green leaf with vertical vein split
    m.insert(
        "mongodb",
        IconDef {
            name: "mongodb",
            default_color: "#13aa52",
            svg_body: r##"<path fill="#13aa52" d="M12 2C11.5 2.7 7 8 7 13.2c0 3.8 2.3 6.8 5 7.8V3.5c0-.6.4-1.1 0-1.5z"/><path fill="#00684a" d="M12 2c.5.7 5 6 5 11.2 0 3.8-2.3 6.8-5 7.8V3.5c0-.6-.4-1.1 0-1.5z"/>"##,
        },
    );

    // DynamoDB: AWS multi-ring cylinder
    m.insert(
        "dynamodb",
        IconDef {
            name: "dynamodb",
            default_color: "#4053d6",
            svg_body: r##"<path fill="#4053d6" d="M12 2C6.5 2 2 4.2 2 7v10c0 2.8 4.5 5 10 5s10-2.2 10-5V7c0-2.8-4.5-5-10-5zm0 2.5c4.7 0 8 1.6 8 2.5s-3.3 2.5-8 2.5-8-1.6-8-2.5 3.3-2.5 8-2.5zM4 10.2c1.8 1.1 4.7 1.8 8 1.8s6.2-.7 8-1.8V12c0 .9-3.3 2.5-8 2.5S4 12.9 4 12v-1.8zm0 5c1.8 1.1 4.7 1.8 8 1.8s6.2-.7 8-1.8V17c0 .9-3.3 2.5-8 2.5S4 17.9 4 17v-1.8z"/>"##,
        },
    );

    // Kafka: Event streaming hub with network circles
    m.insert(
        "kafka",
        IconDef {
            name: "kafka",
            default_color: "#231f20",
            svg_body: r##"<circle cx="12" cy="12" r="3.2" fill="#231f20"/><circle cx="5" cy="6.5" r="2.3" fill="#231f20"/><circle cx="5" cy="17.5" r="2.3" fill="#231f20"/><circle cx="19" cy="6.5" r="2.3" fill="#231f20"/><circle cx="19" cy="17.5" r="2.3" fill="#231f20"/><path stroke="#231f20" stroke-width="1.8" d="M7 7.5l3.3 3.3M7 16.5l3.3-3.3M17 7.5l-3.3 3.3M17 16.5l-3.3-3.3"/>"##,
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
            svg_body: r##"<ellipse cx="12" cy="5.5" rx="8" ry="3" fill="#0284c7"/><path fill="#0284c7" d="M4 5.5v13c0 1.7 3.6 3 8 3s8-1.3 8-3v-13H4zm8 14.5c-3.5 0-6.5-.9-7.5-2V9.8c1.8 1.1 4.5 1.7 7.5 1.7s5.7-.6 7.5-1.7V17c-1 1.1-4 2-7.5 2z"/>"##,
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

/// Renders the vector icon as an elevated badge tile pinned at the top-left boundary of a component card.
///
/// The badge straddles the top border (sitting partly above and partly inside the card),
/// completely freeing the interior of the card for labels and text.
pub fn render_icon_badge_svg(key: &str, card_x: f64, card_y: f64, is_dark: bool) -> Option<String> {
    let icon = get_icon(key)?;
    let badge_size = 20.0;
    let icon_size = 13.0;
    let badge_x = card_x + 8.0;
    let badge_y = card_y - 10.0; // Straddles top border: 10px above card, 10px inside card
    let icon_x = badge_x + (badge_size - icon_size) / 2.0;
    let icon_y = badge_y + (badge_size - icon_size) / 2.0;
    let scale = icon_size / 24.0;

    let bg_color = if is_dark { "#1e293b" } else { "#ffffff" };
    let border_color = if is_dark { "#475569" } else { "#cbd5e1" };

    Some(format!(
        r##"<g class="node-icon-badge">
  <rect x="{badge_x:.1}" y="{badge_y:.1}" width="{badge_size:.1}" height="{badge_size:.1}" rx="5" fill="{bg_color}" stroke="{border_color}" stroke-width="1.2" filter="url(#card-shadow)"/>
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

    #[test]
    fn test_render_icon_badge_svg() {
        let badge = render_icon_badge_svg("rust", 100.0, 200.0, false).expect("badge rendering");
        assert!(badge.contains("node-icon-badge"));
        assert!(badge.contains("x=\"108.0\""));
        assert!(badge.contains("y=\"190.0\"")); // 200 - 10 = 190
        assert!(badge.contains("#ce422b"));
    }
}
