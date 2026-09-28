//! Binary-level integration tests for the `rdg` CLI.
//!
//! Runs the actual compiled binary via `std::process::Command` (no test-only
//! dev-dependency needed — Cargo provides `CARGO_BIN_EXE_rdg` to integration tests in a
//! package with a `[[bin]]` target). These exist because Phase G found zero test coverage
//! at the binary level: every other test in the workspace exercises a library crate
//! directly and never goes through argument parsing, stdin/stdout, or process exit codes.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_rdg")
}

fn repo_doc(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs")
        .join(name)
}

fn run_with_stdin(args: &[&str], stdin: &str) -> std::process::Output {
    let mut child = Command::new(bin())
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn rdg binary");
    child
        .stdin
        .take()
        .expect("child stdin")
        .write_all(stdin.as_bytes())
        .expect("failed to write to child stdin");
    child.wait_with_output().expect("failed to wait on child")
}

#[test]
fn test_example_flag_prints_valid_yaml_to_stdout() {
    let output = Command::new(bin())
        .arg("--example")
        .output()
        .expect("run rdg --example");
    assert!(output.status.success());
    let yaml = String::from_utf8(output.stdout).expect("stdout should be UTF-8");
    assert!(yaml.contains("nodes:"));
    assert!(yaml.contains("edges:"));
}

#[test]
fn test_schema_flag_prints_json_schema_to_stdout() {
    let output = Command::new(bin())
        .arg("--schema")
        .output()
        .expect("run rdg --schema");
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).expect("stdout should be UTF-8");
    assert!(text.contains("\"$schema\""));
    assert!(text.trim_start().starts_with('{'));
}

#[test]
fn test_stdin_yaml_renders_svg_successfully() {
    let out_path = std::env::temp_dir().join("rdg_cli_test_stdin.svg");
    let yaml = "nodes:\n  - id: n1\n    label: Client\n  - id: n2\n    label: Server\nedges:\n  - from: n1\n    to: n2\n";
    let output = run_with_stdin(
        &["-o", out_path.to_str().unwrap(), "--svg-engine", "native"],
        yaml,
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let content = std::fs::read_to_string(&out_path).expect("output file should exist");
    assert!(content.starts_with("<?xml"));
    assert!(content.contains("<svg"));
    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn test_real_architecture_fixture_renders_cleanly() {
    let out_path = std::env::temp_dir().join("rdg_cli_test_architecture.svg");
    let output = Command::new(bin())
        .args([
            "-i",
            repo_doc("architecture.yaml").to_str().unwrap(),
            "-o",
            out_path.to_str().unwrap(),
            "--svg-engine",
            "native",
        ])
        .output()
        .expect("run rdg on architecture.yaml");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // A real, reasonable diagram should self-review clean: the self-review loop may need a
    // wider retry pass (that's what it's for), but no anomaly may remain in the result.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("remains in the best attempt"),
        "anomalies remain after the retry budget: {stderr}"
    );
    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn test_invalid_yaml_exits_nonzero_with_error_on_stderr() {
    let out_path = std::env::temp_dir().join("rdg_cli_test_invalid.svg");
    let output = run_with_stdin(&["-o", out_path.to_str().unwrap()], "not: valid: yaml: {{");
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stderr).is_empty());
    assert!(
        !out_path.exists(),
        "no output file should be written on a parse error"
    );
}

#[test]
fn test_unknown_edge_endpoint_exits_nonzero() {
    let out_path = std::env::temp_dir().join("rdg_cli_test_unknown_edge.svg");
    let yaml = "nodes:\n  - id: n1\n    label: A\nedges:\n  - from: n1\n    to: ghost\n";
    let output = run_with_stdin(&["-o", out_path.to_str().unwrap()], yaml);
    assert!(!output.status.success());
    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn test_strict_flag_fails_when_anomalies_remain() {
    // An explicit size far too small for its content is a `LabelOverflow` anomaly that
    // widening spacing can never fix — exactly the case `--strict` exists to catch.
    let out_path = std::env::temp_dir().join("rdg_cli_test_strict.svg");
    let yaml = "nodes:\n  - id: n1\n    label: \"A very very very long label indeed\"\n    width: 10\n    height: 10\n  - id: n2\n    label: B\nedges:\n  - from: n1\n    to: n2\n";

    let lenient = run_with_stdin(
        &["-o", out_path.to_str().unwrap(), "--svg-engine", "native"],
        yaml,
    );
    assert!(
        lenient.status.success(),
        "without --strict, rdg should still write its best attempt"
    );
    assert!(out_path.exists());
    let _ = std::fs::remove_file(&out_path);

    let strict = run_with_stdin(
        &[
            "-o",
            out_path.to_str().unwrap(),
            "--svg-engine",
            "native",
            "--strict",
        ],
        yaml,
    );
    assert!(
        !strict.status.success(),
        "--strict should exit non-zero when anomalies remain"
    );
    // --strict still writes the best attempt — it only changes the exit code.
    assert!(out_path.exists());
    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn test_strict_flag_succeeds_on_a_clean_diagram() {
    let out_path = std::env::temp_dir().join("rdg_cli_test_strict_clean.svg");
    let yaml = "nodes:\n  - id: n1\n    label: A\n  - id: n2\n    label: B\nedges:\n  - from: n1\n    to: n2\n";
    let output = run_with_stdin(
        &[
            "-o",
            out_path.to_str().unwrap(),
            "--svg-engine",
            "native",
            "--strict",
        ],
        yaml,
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn test_output_extension_selects_drawio_format() {
    let out_path = std::env::temp_dir().join("rdg_cli_test_format.drawio");
    let yaml = "nodes:\n  - id: n1\n    label: A\n  - id: n2\n    label: B\nedges:\n  - from: n1\n    to: n2\n";
    let output = run_with_stdin(&["-o", out_path.to_str().unwrap()], yaml);
    assert!(output.status.success());
    let content = std::fs::read_to_string(&out_path).expect("output file should exist");
    assert!(content.contains("<mxfile"));
    let _ = std::fs::remove_file(&out_path);
}

// -----------------------------------------------------------------------
// Topology dispatcher (`--layout auto|sugiyama|force|fcose`)
// -----------------------------------------------------------------------

#[test]
fn test_layout_force_renders_successfully() {
    let out_path = std::env::temp_dir().join("rdg_cli_test_layout_force.svg");
    let yaml = "nodes:\n  - id: a\n    label: A\n  - id: b\n    label: B\nedges:\n  - from: a\n    to: b\n";
    let output = run_with_stdin(&["-o", out_path.to_str().unwrap(), "--svg-engine", "native", "--layout", "force"], yaml);
    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn test_layout_fcose_renders_successfully() {
    let out_path = std::env::temp_dir().join("rdg_cli_test_layout_fcose.svg");
    let yaml = "nodes:\n  - id: a\n    label: A\n  - id: b\n    label: B\nedges:\n  - from: a\n    to: b\n";
    let output = run_with_stdin(&["-o", out_path.to_str().unwrap(), "--svg-engine", "native", "--layout", "fcose"], yaml);
    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn test_layout_auto_on_dense_graph_reports_force_directed() {
    let out_path = std::env::temp_dir().join("rdg_cli_test_layout_auto_dense.svg");
    // 5 nodes, 12 edges: density 2.4, above the dispatcher's dense threshold.
    let yaml = "nodes:\n  - {id: a, label: A}\n  - {id: b, label: B}\n  - {id: c, label: C}\n  - {id: d, label: D}\n  - {id: e, label: E}\nedges:\n  - {from: a, to: b}\n  - {from: a, to: c}\n  - {from: a, to: d}\n  - {from: b, to: c}\n  - {from: b, to: d}\n  - {from: b, to: e}\n  - {from: c, to: d}\n  - {from: c, to: e}\n  - {from: d, to: e}\n  - {from: a, to: e}\n  - {from: d, to: a}\n  - {from: e, to: c}\n";
    let output = run_with_stdin(&["-o", out_path.to_str().unwrap(), "--svg-engine", "native"], yaml);
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("force-directed"), "expected auto-dispatch to force-directed, got: {stderr}");
    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn test_layout_auto_on_disconnected_graph_reports_fcose() {
    let out_path = std::env::temp_dir().join("rdg_cli_test_layout_auto_disconnected.svg");
    let yaml = "nodes:\n  - {id: a, label: A}\n  - {id: b, label: B}\n  - {id: c, label: C}\n  - {id: d, label: D}\nedges:\n  - {from: a, to: b}\n  - {from: c, to: d}\n";
    let output = run_with_stdin(&["-o", out_path.to_str().unwrap(), "--svg-engine", "native"], yaml);
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("fcose"), "expected auto-dispatch to fcose, got: {stderr}");
    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn test_explicit_layout_flag_suppresses_dispatch_report() {
    let out_path = std::env::temp_dir().join("rdg_cli_test_layout_explicit_quiet.svg");
    let yaml = "nodes:\n  - id: a\n    label: A\n  - id: b\n    label: B\nedges:\n  - from: a\n    to: b\n";
    let output = run_with_stdin(&["-o", out_path.to_str().unwrap(), "--svg-engine", "native", "--layout", "force"], yaml);
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("dispatch:"), "an explicit --layout choice shouldn't print a dispatch report: {stderr}");
    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn test_force_and_fcose_layouts_are_deterministic_across_runs() {
    let yaml = "nodes:\n  - {id: a, label: A}\n  - {id: b, label: B}\n  - {id: c, label: C}\n  - {id: d, label: D}\nedges:\n  - {from: a, to: b}\n  - {from: b, to: c}\n  - {from: c, to: d}\n  - {from: d, to: a}\n  - {from: a, to: c}\n";
    for layout in ["force", "fcose"] {
        let out1 = std::env::temp_dir().join(format!("rdg_cli_test_det_{layout}_1.svg"));
        let out2 = std::env::temp_dir().join(format!("rdg_cli_test_det_{layout}_2.svg"));
        run_with_stdin(&["-o", out1.to_str().unwrap(), "--svg-engine", "native", "--layout", layout], yaml);
        run_with_stdin(&["-o", out2.to_str().unwrap(), "--svg-engine", "native", "--layout", layout], yaml);
        let c1 = std::fs::read_to_string(&out1).expect("first run should produce output");
        let c2 = std::fs::read_to_string(&out2).expect("second run should produce output");
        assert_eq!(c1, c2, "{layout} layout should be byte-for-byte deterministic across runs");
        let _ = std::fs::remove_file(&out1);
        let _ = std::fs::remove_file(&out2);
    }
}

/// Regression test for a real determinism bug: `ideal_edge_length` (in `rdg-layout`)
/// used to sum node diagonals via `HashMap::values()`, whose iteration order is
/// randomized per *process* — a non-associative float summation order that fed into
/// fCoSE's spectral MDS draft, which is sensitive enough to tiny input differences
/// (particularly with near-symmetric/disconnected structure) to land on a visibly
/// different layout. The trivial 4-node single-component graph in the test above
/// never triggered it — the summation-order noise was too small to flip anything
/// discrete for a graph that simple. A multi-component graph with repeated,
/// near-symmetric per-component shape (three near-identical 2-node components) is
/// exactly the kind of case where it did: confirmed directly, several consecutive
/// runs of the unfixed binary on a real sample of this shape produced distinct
/// outputs more often than not.
#[test]
fn test_fcose_disconnected_components_are_deterministic_across_many_runs() {
    let yaml = "nodes:\n  - {id: a1, label: A1}\n  - {id: a2, label: A2}\n  - {id: b1, label: B1}\n  - {id: b2, label: B2}\n  - {id: c1, label: C1}\n  - {id: c2, label: C2}\nedges:\n  - {from: a1, to: a2}\n  - {from: b1, to: b2}\n  - {from: c1, to: c2}\n";
    let mut outputs = std::collections::HashSet::new();
    for i in 0..6 {
        let out_path =
            std::env::temp_dir().join(format!("rdg_cli_test_det_fcose_disconnected_{i}.svg"));
        run_with_stdin(
            &["-o", out_path.to_str().unwrap(), "--svg-engine", "native", "--layout", "fcose"],
            yaml,
        );
        let content = std::fs::read_to_string(&out_path).expect("run should produce output");
        outputs.insert(content);
        let _ = std::fs::remove_file(&out_path);
    }
    assert_eq!(
        outputs.len(),
        1,
        "fCoSE layout on a disconnected multi-component graph must be byte-for-byte \
         deterministic across process runs, got {} distinct outputs across 6 runs",
        outputs.len()
    );
}

/// Whitespace regression guard over every bundled diagram: no arrow may shrink to a
/// stub of its own arrowhead, and no two connection points may coincide on a face. Reads
/// the `RDG_DEBUG_SPACING` line rdg prints (see `rdg_render_core::review::spacing_metrics`).
#[test]
fn test_bundled_diagrams_keep_arrows_and_ports_readable() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut checked = 0;
    // Whole-suite polish budgets (measured after polish: 1 micro-jog, 24 crossings across the
    // bundled diagrams; unpolished it was 11 and 54). A little headroom keeps unrelated
    // layout changes from tripping this, while a real regression in polish still does.
    let (mut total_jogs, mut total_crossings) = (0.0_f64, 0.0_f64);
    for dir in ["examples", "docs"] {
        let mut files: Vec<_> = std::fs::read_dir(root.join(dir))
            .expect("read fixture dir")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("yaml"))
            .collect();
        files.sort();
        for file in files {
            let out_path = std::env::temp_dir().join(format!(
                "rdg_cli_test_spacing_{}.svg",
                file.file_stem().unwrap().to_string_lossy()
            ));
            let output = Command::new(bin())
                .env("RDG_DEBUG_SPACING", "1")
                .args(["-i", file.to_str().unwrap(), "-o", out_path.to_str().unwrap(), "--svg-engine", "native"])
                .output()
                .expect("run rdg");
            assert!(output.status.success(), "{}: {}", file.display(), String::from_utf8_lossy(&output.stderr));
            let stderr = String::from_utf8_lossy(&output.stderr);
            let line = stderr.lines().find(|l| l.starts_with("spacing:")).expect("spacing line");
            let field = |name: &str| -> Option<f64> {
                line.split_whitespace()
                    .find_map(|t| t.strip_prefix(name))
                    .and_then(|v| v.parse().ok())
            };
            // "-" (nothing to measure, e.g. a sequence diagram) parses to None and is skipped.
            if let Some(arrow) = field("arrow=") {
                assert!(arrow >= 20.0, "{}: shortest arrow is {arrow}px — {line}", file.display());
            }
            if let Some(pitch) = field("port_pitch=") {
                assert!(pitch >= 8.0, "{}: ports only {pitch}px apart — {line}", file.display());
            }
            total_jogs += field("jogs=").unwrap_or(0.0);
            total_crossings += field("crossings=").unwrap_or(0.0);
            let _ = std::fs::remove_file(&out_path);
            checked += 1;
        }
    }
    assert!(checked >= 8, "expected to check the bundled fixtures, checked {checked}");
    assert!(total_jogs <= 3.0, "micro-jogs across bundled diagrams: {total_jogs} (budget 3)");
    assert!(total_crossings <= 30.0, "crossings across bundled diagrams: {total_crossings} (budget 30)");
}
